use super::*;

// --- SessionCache tests ---

fn cache() -> SessionCache {
    SessionCache::new("test-session")
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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
                session_name: "test-session".to_string(),
                dead: true,
                dead_status: Some(1),
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
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

// ── multi-session cache (foreign pane exclusion) ──────────────────────────

fn test_pane(session: &str) -> PaneState {
    PaneState {
        buffer: String::new(),
        summary: String::new(),
        current_cmd: String::new(),
        current_path: String::new(),
        pane_title: String::new(),
        last_updated: std::time::Instant::now(),
        scroll_position: 0,
        history_size: 0,
        in_copy_mode: false,
        synchronized: false,
        window_name: String::new(),
        session_name: session.to_string(),
        dead: false,
        dead_status: None,
        last_activity: 0,
        start_cmd: String::new(),
        pane_index: 0,
        shell_pid: 0,
        status: crate::tmux::status::PaneStatus::Idle(0),
    }
}

#[test]
fn pane_map_excludes_foreign_session_panes() {
    let c = SessionCache::new("home");
    {
        let mut panes = c.panes.write().unwrap_or_log();
        panes.insert("%1".to_string(), test_pane("home"));
        panes.insert("%9".to_string(), {
            let mut p = test_pane("other");
            p.window_name = "editor".to_string();
            p
        });
    }
    let summary = c.pane_map_summary(None);
    assert!(
        summary.contains("%1"),
        "home pane should appear in map, got: {summary}"
    );
    assert!(
        !summary.contains("%9"),
        "foreign pane must not appear in map, got: {summary}"
    );
}

#[test]
fn labeled_context_excludes_foreign_session_panes() {
    let c = SessionCache::new("home");
    {
        let mut panes = c.panes.write().unwrap_or_log();
        panes.insert("%1".to_string(), test_pane("home"));
        panes.insert("%9".to_string(), {
            let mut p = test_pane("other");
            p.window_name = "editor".to_string();
            p
        });
    }
    let ctx = c.get_labeled_context(None, None);
    assert!(
        ctx.contains("%1"),
        "home pane should appear in context, got: {ctx}"
    );
    assert!(
        !ctx.contains("%9"),
        "foreign pane must not appear in context, got: {ctx}"
    );
}

#[test]
fn is_home_pane_rejects_foreign_session_pane() {
    let c = SessionCache::new("home");
    {
        let mut panes = c.panes.write().unwrap_or_log();
        panes.insert("%1".to_string(), test_pane("home"));
        panes.insert("%9".to_string(), test_pane("other"));
    }
    assert!(c.is_home_pane("%1"), "home pane should be accepted");
    assert!(!c.is_home_pane("%9"), "foreign pane should be rejected");
    assert!(!c.is_home_pane("%99"), "unknown pane should be rejected");
}

#[test]
fn evict_missing_removes_closed_panes() {
    let c = SessionCache::new("home");
    {
        let mut panes = c.panes.write().unwrap_or_log();
        panes.insert("%1".to_string(), test_pane("home"));
        panes.insert("%2".to_string(), test_pane("home"));
    }
    let mut live = std::collections::HashSet::new();
    live.insert("%1".to_string());
    c.evict_missing(&live);
    let panes = c.panes.read().unwrap_or_log();
    assert!(panes.contains_key("%1"), "%1 should remain");
    assert!(!panes.contains_key("%2"), "%2 should be evicted");
}

#[test]
fn evict_missing_ignores_empty_snapshot() {
    let c = SessionCache::new("home");
    {
        let mut panes = c.panes.write().unwrap_or_log();
        panes.insert("%1".to_string(), test_pane("home"));
        panes.insert("%2".to_string(), test_pane("home"));
    }
    let empty: std::collections::HashSet<String> = std::collections::HashSet::new();
    c.evict_missing(&empty);
    let panes = c.panes.read().unwrap_or_log();
    assert!(
        panes.contains_key("%1"),
        "%1 should remain after empty snapshot"
    );
    assert!(
        panes.contains_key("%2"),
        "%2 should remain after empty snapshot"
    );
}

// ── list_panes foreign session exclusion is in pane.rs tests ──────────────

// ── ContextScope + window_in_scope ────────────────────────────────────────

#[test]
fn window_in_scope_session_and_all_admit_everything() {
    assert!(
        window_in_scope(ContextScope::Session, "other", Some("main")),
        "Session should admit any window"
    );
    assert!(
        window_in_scope(ContextScope::All, "other", Some("main")),
        "All should admit any window"
    );
}

#[test]
fn window_in_scope_window_rejects_other_windows() {
    assert!(
        window_in_scope(ContextScope::Window, "main", Some("main")),
        "Window scope should keep the chat window"
    );
    assert!(
        !window_in_scope(ContextScope::Window, "other", Some("main")),
        "Window scope should reject a different window"
    );
    assert!(
        window_in_scope(ContextScope::Window, "any", None),
        "Window scope with None chat_window admits everything"
    );
}

// ── get_labeled_context_scoped ────────────────────────────────────────────

#[test]
fn labeled_context_window_scope_excludes_other_windows() {
    let c = cache();
    {
        let mut panes = c.panes.write().unwrap();
        panes.insert(
            "%1".to_string(),
            PaneState {
                buffer: "chat content".to_string(),
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
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
            },
        );
        panes.insert(
            "%2".to_string(),
            PaneState {
                buffer: "other content".to_string(),
                summary: "editor".to_string(),
                current_cmd: "nvim".to_string(),
                current_path: "/srv/app".to_string(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 0,
                in_copy_mode: false,
                synchronized: false,
                window_name: "other".to_string(),
                session_name: "test-session".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Running,
            },
        );
    }
    let ctx_window = c.get_labeled_context_scoped(Some("%1"), Some("%1"), ContextScope::Window);
    assert!(
        !ctx_window.contains("%2"),
        "Window scope should exclude panes in other windows, got: {ctx_window}"
    );
    let ctx_session = c.get_labeled_context_scoped(Some("%1"), Some("%1"), ContextScope::Session);
    assert!(
        ctx_session.contains("%2"),
        "Session scope should include panes in other windows, got: {ctx_session}"
    );
}

#[test]
fn labeled_context_all_scope_lists_foreign_panes() {
    let c = SessionCache::new("home");
    {
        let mut panes = c.panes.write().unwrap();
        panes.insert(
            "%1".to_string(),
            PaneState {
                buffer: "home content".to_string(),
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
                session_name: "home".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
            },
        );
        panes.insert(
            "%9".to_string(),
            PaneState {
                buffer: String::new(),
                summary: "editor".to_string(),
                current_cmd: "nvim".to_string(),
                current_path: "/srv/app".to_string(),
                pane_title: String::new(),
                last_updated: std::time::Instant::now(),
                scroll_position: 0,
                history_size: 0,
                in_copy_mode: false,
                synchronized: false,
                window_name: "editor".to_string(),
                session_name: "other".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 1,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Running,
            },
        );
    }
    let ctx_all = c.get_labeled_context_scoped(None, None, ContextScope::All);
    assert!(
        ctx_all.contains("FOREIGN SESSION PANE"),
        "All scope should list foreign panes, got: {ctx_all}"
    );
    assert!(
        ctx_all.contains("%9"),
        "All scope should include foreign pane id, got: {ctx_all}"
    );
    let ctx_session = c.get_labeled_context_scoped(None, None, ContextScope::Session);
    assert!(
        !ctx_session.contains("FOREIGN SESSION PANE"),
        "Session scope should not list foreign panes, got: {ctx_session}"
    );
    assert!(
        !ctx_session.contains("%9"),
        "Session scope should not include foreign pane id, got: {ctx_session}"
    );
}

#[test]
fn labeled_context_session_scope_omits_foreign_header_when_none() {
    let c = SessionCache::new("home");
    {
        let mut panes = c.panes.write().unwrap();
        panes.insert(
            "%1".to_string(),
            PaneState {
                buffer: "home content".to_string(),
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
                session_name: "home".to_string(),
                dead: false,
                dead_status: None,
                last_activity: 0,
                start_cmd: String::new(),
                pane_index: 0,
                shell_pid: 0,
                status: crate::tmux::status::PaneStatus::Idle(0),
            },
        );
    }
    let ctx_all = c.get_labeled_context_scoped(None, None, ContextScope::All);
    assert!(
        !ctx_all.contains("FOREIGN SESSION PANE"),
        "All scope with no foreign panes should not emit foreign header, got: {ctx_all}"
    );
}
