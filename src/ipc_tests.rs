use super::*;

fn roundtrip_req(req: &Request) -> Request {
    let json = serde_json::to_string(req).expect("serialize");
    serde_json::from_str(&json).expect("deserialize")
}

fn roundtrip_resp(resp: &Response) -> Response {
    let json = serde_json::to_string(resp).expect("serialize");
    serde_json::from_str(&json).expect("deserialize")
}

// ── Request round-trips ──────────────────────────────────────────────────

#[test]
fn request_ping_roundtrip() {
    assert!(matches!(roundtrip_req(&Request::Ping), Request::Ping));
}

#[test]
fn request_shutdown_roundtrip() {
    assert!(matches!(
        roundtrip_req(&Request::Shutdown),
        Request::Shutdown
    ));
}

#[test]
fn request_refresh_roundtrip() {
    assert!(matches!(roundtrip_req(&Request::Refresh), Request::Refresh));
}

#[test]
fn request_ask_roundtrip() {
    let req = Request::Ask {
        query: "what is load avg?".to_string(),
        tmux_pane: Some("%3".to_string()),
        session_id: Some("deadbeef".to_string()),
        chat_pane: Some("%4".to_string()),
        prompt: Some("sre".to_string()),
        chat_width: Some(54),
        tmux_session: Some("mysession".to_string()),
        target_pane: Some("%1".to_string()),
        model: Some("opus".to_string()),
    };
    match roundtrip_req(&req) {
        Request::Ask {
            query,
            tmux_pane,
            session_id,
            chat_pane,
            prompt,
            chat_width,
            tmux_session,
            target_pane,
            model,
        } => {
            assert_eq!(query, "what is load avg?");
            assert_eq!(tmux_pane, Some("%3".to_string()));
            assert_eq!(session_id, Some("deadbeef".to_string()));
            assert_eq!(chat_pane, Some("%4".to_string()));
            assert_eq!(prompt, Some("sre".to_string()));
            assert_eq!(chat_width, Some(54));
            assert_eq!(tmux_session, Some("mysession".to_string()));
            assert_eq!(target_pane, Some("%1".to_string()));
            assert_eq!(model, Some("opus".to_string()));
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_ask_optional_fields_none() {
    let req = Request::Ask {
        query: "hi".to_string(),
        tmux_pane: None,
        session_id: None,
        chat_pane: None,
        prompt: None,
        chat_width: None,
        tmux_session: None,
        target_pane: None,
        model: None,
    };
    match roundtrip_req(&req) {
        Request::Ask {
            tmux_pane,
            session_id,
            chat_pane,
            prompt,
            chat_width,
            ..
        } => {
            assert!(tmux_pane.is_none());
            assert!(session_id.is_none());
            assert!(chat_pane.is_none());
            assert!(prompt.is_none());
            assert!(chat_width.is_none());
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_ask_backward_compat_no_prompt_field() {
    // Simulate a message from an old client that omits the `prompt` field.
    let json = r#"{"Ask":{"query":"hi","tmux_pane":null,"session_id":null,"chat_pane":null}}"#;
    let parsed: Request = serde_json::from_str(json).expect("backward-compat deserialize");
    match parsed {
        Request::Ask { prompt, .. } => assert!(prompt.is_none()),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_tool_call_response_roundtrip() {
    let req = Request::ToolCallResponse {
        id: "tc_1".to_string(),
        approved: true,
        user_message: None,
    };
    match roundtrip_req(&req) {
        Request::ToolCallResponse {
            id,
            approved,
            user_message,
        } => {
            assert_eq!(id, "tc_1");
            assert!(approved);
            assert!(user_message.is_none());
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_tool_call_response_with_user_message_roundtrip() {
    let req = Request::ToolCallResponse {
        id: "tc_2".to_string(),
        approved: false,
        user_message: Some("don't do that, try a safer approach".to_string()),
    };
    match roundtrip_req(&req) {
        Request::ToolCallResponse {
            id,
            approved,
            user_message,
        } => {
            assert_eq!(id, "tc_2");
            assert!(!approved);
            assert_eq!(
                user_message.as_deref(),
                Some("don't do that, try a safer approach")
            );
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_tool_call_response_backward_compat_no_user_message() {
    // Old clients omit user_message; default should be None.
    let json = r#"{"ToolCallResponse":{"id":"tc_3","approved":false}}"#;
    let parsed: Request = serde_json::from_str(json).expect("backward-compat deserialize");
    match parsed {
        Request::ToolCallResponse { user_message, .. } => assert!(user_message.is_none()),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_credential_response_roundtrip() {
    let req = Request::CredentialResponse {
        id: "tc_2".to_string(),
        credential: "hunter2".to_string(),
    };
    match roundtrip_req(&req) {
        Request::CredentialResponse { id, credential } => {
            assert_eq!(id, "tc_2");
            assert_eq!(credential, "hunter2");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_notify_activity_roundtrip() {
    let req = Request::NotifyActivity {
        pane_id: "%3".to_string(),
        hook_index: 42,
        session_name: "test_session".to_string(),
    };
    match roundtrip_req(&req) {
        Request::NotifyActivity { pane_id, .. } => {
            assert_eq!(pane_id, "%3");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_notify_complete_roundtrip() {
    let req = Request::NotifyComplete {
        pane_id: "%5".to_string(),
        exit_code: 42,
        session_name: "test_session".to_string(),
    };
    match roundtrip_req(&req) {
        Request::NotifyComplete {
            pane_id, exit_code, ..
        } => {
            assert_eq!(pane_id, "%5");
            assert_eq!(exit_code, 42);
        }
        _ => panic!("wrong variant"),
    }
}

// ── Response round-trips ─────────────────────────────────────────────────

#[test]
fn response_ok_roundtrip() {
    assert!(matches!(roundtrip_resp(&Response::Ok), Response::Ok));
}

#[test]
fn response_error_roundtrip() {
    let resp = Response::Error("something broke".to_string());
    match roundtrip_resp(&resp) {
        Response::Error(msg) => assert_eq!(msg, "something broke"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_session_info_roundtrip() {
    let resp = Response::SessionInfo {
        message_count: 7,
        turn_count: 3,
    };
    match roundtrip_resp(&resp) {
        Response::SessionInfo {
            message_count,
            turn_count,
        } => {
            assert_eq!(message_count, 7);
            assert_eq!(turn_count, 3);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_token_roundtrip() {
    let resp = Response::Token("Hello".to_string());
    match roundtrip_resp(&resp) {
        Response::Token(t) => assert_eq!(t, "Hello"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_tool_call_prompt_roundtrip() {
    let resp = Response::ToolCallPrompt {
        id: "tc_3".to_string(),
        command: "ls -la".to_string(),
        background: false,
        target_pane: Some("%5".to_string()),
    };
    match roundtrip_resp(&resp) {
        Response::ToolCallPrompt {
            id,
            command,
            background,
            target_pane,
        } => {
            assert_eq!(id, "tc_3");
            assert_eq!(command, "ls -la");
            assert!(!background);
            assert_eq!(target_pane, Some("%5".to_string()));
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_tool_call_prompt_no_target_pane_roundtrip() {
    // Older daemons omit target_pane; default should be None.
    let json = r#"{"ToolCallPrompt":{"id":"tc_3","command":"ls -la","background":false}}"#;
    let parsed: Response = serde_json::from_str(json).expect("backward-compat deserialize");
    match parsed {
        Response::ToolCallPrompt { target_pane, .. } => assert!(target_pane.is_none()),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_credential_prompt_roundtrip() {
    let resp = Response::CredentialPrompt {
        id: "tc_4".to_string(),
        prompt: "[sudo] password for alice:".to_string(),
    };
    match roundtrip_resp(&resp) {
        Response::CredentialPrompt { id, prompt } => {
            assert_eq!(id, "tc_4");
            assert_eq!(prompt, "[sudo] password for alice:");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_tool_result_roundtrip() {
    let resp = Response::ToolResult("output here".to_string());
    match roundtrip_resp(&resp) {
        Response::ToolResult(s) => assert_eq!(s, "output here"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_pane_select_response_roundtrip() {
    let req = Request::PaneSelectResponse {
        id: "ps_1".to_string(),
        pane_id: "%3".to_string(),
    };
    match roundtrip_req(&req) {
        Request::PaneSelectResponse { id, pane_id } => {
            assert_eq!(id, "ps_1");
            assert_eq!(pane_id, "%3");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_pane_select_prompt_roundtrip() {
    let resp = Response::PaneSelectPrompt {
        id: "ps_2".to_string(),
        panes: vec![
            PaneInfo {
                id: "%1".to_string(),
                current_cmd: "bash".to_string(),
                summary: "idle shell".to_string(),
            },
            PaneInfo {
                id: "%3".to_string(),
                current_cmd: "vim".to_string(),
                summary: "editing file".to_string(),
            },
        ],
    };
    match roundtrip_resp(&resp) {
        Response::PaneSelectPrompt { id, panes } => {
            assert_eq!(id, "ps_2");
            assert_eq!(panes.len(), 2);
            assert_eq!(panes[0].id, "%1");
            assert_eq!(panes[0].current_cmd, "bash");
            assert_eq!(panes[1].id, "%3");
            assert_eq!(panes[1].current_cmd, "vim");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_script_write_response_roundtrip() {
    let req = Request::ScriptWriteResponse {
        id: "sw_1".to_string(),
        approved: true,
    };
    match roundtrip_req(&req) {
        Request::ScriptWriteResponse { id, approved } => {
            assert_eq!(id, "sw_1");
            assert!(approved);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_script_write_prompt_roundtrip() {
    // New file: no existing content
    let resp = Response::ScriptWritePrompt {
        id: "sw_2".to_string(),
        script_name: "check-disk.sh".to_string(),
        content: "#!/bin/bash\ndf -h".to_string(),
        existing_content: None,
    };
    match roundtrip_resp(&resp) {
        Response::ScriptWritePrompt {
            id,
            script_name,
            content,
            existing_content,
        } => {
            assert_eq!(id, "sw_2");
            assert_eq!(script_name, "check-disk.sh");
            assert!(content.contains("df -h"));
            assert!(existing_content.is_none());
        }
        _ => panic!("wrong variant"),
    }

    // Modified file: existing content provided
    let resp2 = Response::ScriptWritePrompt {
        id: "sw_3".to_string(),
        script_name: "check-disk.sh".to_string(),
        content: "#!/bin/bash\ndf -h\necho done".to_string(),
        existing_content: Some("#!/bin/bash\ndf -h".to_string()),
    };
    match roundtrip_resp(&resp2) {
        Response::ScriptWritePrompt {
            existing_content, ..
        } => {
            assert!(existing_content.is_some());
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_runbook_write_prompt_roundtrip() {
    // New runbook: no existing content
    let resp = Response::RunbookWritePrompt {
        id: "rw_1".to_string(),
        runbook_name: "disk-alert".to_string(),
        content: "# Runbook: disk-alert\n## Alert Criteria\ndf -h".to_string(),
        existing_content: None,
    };
    match roundtrip_resp(&resp) {
        Response::RunbookWritePrompt {
            id,
            runbook_name,
            content,
            existing_content,
        } => {
            assert_eq!(id, "rw_1");
            assert_eq!(runbook_name, "disk-alert");
            assert!(content.contains("df -h"));
            assert!(existing_content.is_none());
        }
        _ => panic!("wrong variant"),
    }

    // Modified runbook: existing content provided
    let resp2 = Response::RunbookWritePrompt {
        id: "rw_2".to_string(),
        runbook_name: "disk-alert".to_string(),
        content: "# Runbook: disk-alert\n## Alert Criteria\ndf -h\nnew line".to_string(),
        existing_content: Some("# Runbook: disk-alert\n## Alert Criteria\ndf -h".to_string()),
    };
    match roundtrip_resp(&resp2) {
        Response::RunbookWritePrompt {
            existing_content, ..
        } => {
            assert!(existing_content.is_some());
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_schedule_write_response_roundtrip() {
    let req = Request::ScheduleWriteResponse {
        id: "sch_1".to_string(),
        approved: true,
    };
    match roundtrip_req(&req) {
        Request::ScheduleWriteResponse { id, approved } => {
            assert_eq!(id, "sch_1");
            assert!(approved);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_schedule_write_prompt_roundtrip() {
    let resp = Response::ScheduleWritePrompt {
        id: "sch_2".to_string(),
        name: "MyJob".to_string(),
        kind: "every 5m".to_string(),
        action: "echo Hello".to_string(),
    };
    match roundtrip_resp(&resp) {
        Response::ScheduleWritePrompt {
            id,
            name,
            kind,
            action,
        } => {
            assert_eq!(id, "sch_2");
            assert_eq!(name, "MyJob");
            assert_eq!(kind, "every 5m");
            assert_eq!(action, "echo Hello");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_schedule_list_roundtrip() {
    let resp = Response::ScheduleList {
        jobs: vec![ScheduleListItem {
            id: "job-1".to_string(),
            name: "disk-check".to_string(),
            kind: "every 5m".to_string(),
            action: "cmd: df -h".to_string(),
            status: "pending".to_string(),
            last_run: None,
            next_run: Some("2026-03-01 12:00 UTC".to_string()),
        }],
    };
    match roundtrip_resp(&resp) {
        Response::ScheduleList { jobs } => {
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].name, "disk-check");
            assert_eq!(jobs[0].next_run, Some("2026-03-01 12:00 UTC".to_string()));
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_script_list_roundtrip() {
    let resp = Response::ScriptList {
        scripts: vec![
            ScriptListItem {
                name: "check-disk.sh".to_string(),
                size: 42,
            },
            ScriptListItem {
                name: "monitor.sh".to_string(),
                size: 128,
            },
        ],
    };
    match roundtrip_resp(&resp) {
        Response::ScriptList { scripts } => {
            assert_eq!(scripts.len(), 2);
            assert_eq!(scripts[0].name, "check-disk.sh");
            assert_eq!(scripts[0].size, 42);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_notify_client_attached_roundtrip() {
    let req = Request::NotifyClientAttached {
        session_name: "dev".to_string(),
    };
    match roundtrip_req(&req) {
        Request::NotifyClientAttached { session_name } => assert_eq!(session_name, "dev"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_notify_client_detached_roundtrip() {
    let req = Request::NotifyClientDetached {
        session_name: "staging".to_string(),
    };
    match roundtrip_req(&req) {
        Request::NotifyClientDetached { session_name } => assert_eq!(session_name, "staging"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_notify_session_closed_roundtrip() {
    let req = Request::NotifySessionClosed {
        session_name: "prod".to_string(),
    };
    match roundtrip_req(&req) {
        Request::NotifySessionClosed { session_name } => assert_eq!(session_name, "prod"),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_notify_resize_roundtrip() {
    let req = Request::NotifyResize {
        width: 220,
        height: 50,
        session_name: "main".to_string(),
    };
    match roundtrip_req(&req) {
        Request::NotifyResize {
            width,
            height,
            session_name,
        } => {
            assert_eq!(width, 220);
            assert_eq!(height, 50);
            assert_eq!(session_name, "main");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn request_status_roundtrip() {
    assert!(matches!(roundtrip_req(&Request::Status), Request::Status));
}

#[test]
fn response_daemon_status_roundtrip() {
    let mut memory_breakdown = std::collections::HashMap::new();
    memory_breakdown.insert("knowledge".to_string(), 3);
    memory_breakdown.insert("incident".to_string(), 1);

    let resp = Response::DaemonStatus {
        uptime_secs: 3661,
        pid: 12345,
        active_sessions: 2,
        total_turns: 42,
        provider: "anthropic".to_string(),
        model: "claude-sonnet-4-6".to_string(),
        available_models: vec!["default".to_string(), "opus".to_string()],
        socket_path: "/tmp/daemoneye.sock".to_string(),
        schedule_count: 3,
        commands_fg_succeeded: 5,
        commands_fg_failed: 1,
        commands_fg_approved: 6,
        commands_fg_denied: 2,
        commands_bg_succeeded: 3,
        commands_bg_failed: 1,
        commands_bg_approved: 4,
        commands_bg_denied: 1,
        commands_sched_succeeded: 2,
        commands_sched_failed: 0,
        webhooks_received: 5,
        webhooks_rejected: 1,
        webhook_url: "http://127.0.0.1:8000/webhook".to_string(),
        runbook_count: 2,
        runbooks_created: 1,
        runbooks_executed: 4,
        runbooks_deleted: 0,
        script_count: 3,
        scripts_created: 2,
        scripts_executed: 6,
        scripts_deleted: 1,
        memories_created: 3,
        memories_recalled: 7,
        memories_deleted: 1,
        schedules_created: 2,
        schedules_executed: 5,
        schedules_deleted: 0,
        ghosts_launched: 1,
        ghosts_active: 0,
        ghosts_completed: 1,
        ghosts_failed: 0,
        active_prompt_tokens: 1000,
        context_window_tokens: 4000,
        recent_commands: vec![RecentCommand {
            id: 1,
            cmd: "ls".to_string(),
            timestamp: "2026-03-20 12:00:00".to_string(),
            mode: "foreground".to_string(),
            approval: "approved".to_string(),
            status: "succeeded".to_string(),
        }],
        memory_breakdown: memory_breakdown.clone(),
        redaction_counts: {
            let mut m = std::collections::HashMap::new();
            m.insert("JWT".to_string(), 3);
            m.insert("Secret".to_string(), 1);
            m
        },
        compactions: 2,
        compaction_ratio: 3.5,
        scripts_approved: 0,
        scripts_denied: 0,
        runbooks_approved: 0,
        runbooks_denied: 0,
        file_edits_approved: 0,
        file_edits_denied: 0,
        limits: LimitsSummary::default(),
    };
    match roundtrip_resp(&resp) {
        Response::DaemonStatus {
            uptime_secs,
            pid,
            active_sessions,
            provider,
            model,
            schedule_count,
            commands_fg_succeeded,
            commands_fg_failed,
            commands_bg_succeeded,
            commands_bg_failed,
            commands_sched_succeeded,
            commands_sched_failed,
            webhooks_received,
            webhooks_rejected,
            webhook_url,
            runbooks_created,
            runbooks_executed,
            runbooks_deleted,
            scripts_created,
            scripts_executed,
            memories_created,
            memories_recalled,
            memories_deleted,
            schedules_created,
            schedules_executed,
            schedules_deleted,
            active_prompt_tokens,
            context_window_tokens,
            recent_commands,
            memory_breakdown: mb,
            redaction_counts: rc,
            ..
        } => {
            assert_eq!(uptime_secs, 3661);
            assert_eq!(pid, 12345);
            assert_eq!(active_sessions, 2);
            assert_eq!(provider, "anthropic");
            assert_eq!(model, "claude-sonnet-4-6");
            assert_eq!(schedule_count, 3);
            assert_eq!(commands_fg_succeeded, 5);
            assert_eq!(commands_fg_failed, 1);
            assert_eq!(commands_bg_succeeded, 3);
            assert_eq!(commands_bg_failed, 1);
            assert_eq!(commands_sched_succeeded, 2);
            assert_eq!(commands_sched_failed, 0);
            assert_eq!(webhooks_received, 5);
            assert_eq!(webhooks_rejected, 1);
            assert_eq!(webhook_url, "http://127.0.0.1:8000/webhook");
            assert_eq!(runbooks_created, 1);
            assert_eq!(runbooks_executed, 4);
            assert_eq!(runbooks_deleted, 0);
            assert_eq!(scripts_created, 2);
            assert_eq!(scripts_executed, 6);
            assert_eq!(memories_created, 3);
            assert_eq!(memories_recalled, 7);
            assert_eq!(memories_deleted, 1);
            assert_eq!(schedules_created, 2);
            assert_eq!(schedules_executed, 5);
            assert_eq!(schedules_deleted, 0);
            assert_eq!(active_prompt_tokens, 1000);
            assert_eq!(context_window_tokens, 4000);
            assert_eq!(recent_commands.len(), 1);
            assert_eq!(mb.len(), 2);
            assert_eq!(rc.get("JWT").copied().unwrap_or(0), 3);
            assert_eq!(rc.get("Secret").copied().unwrap_or(0), 1);
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn invalid_json_returns_error() {
    let result: Result<Request, _> = serde_json::from_str("not json at all");
    assert!(result.is_err());
}

// ── ToolStarted / ToolFinished round-trips ───────────────────────────────

#[test]
fn response_tool_started_roundtrip() {
    let resp = Response::ToolStarted {
        id: "ts_1".to_string(),
        tool: "read_file".to_string(),
        summary: "/etc/hosts grep=\"nameserver\"".to_string(),
    };
    match roundtrip_resp(&resp) {
        Response::ToolStarted { id, tool, summary } => {
            assert_eq!(id, "ts_1");
            assert_eq!(tool, "read_file");
            assert_eq!(summary, "/etc/hosts grep=\"nameserver\"");
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_tool_started_empty_summary_roundtrip() {
    let resp = Response::ToolStarted {
        id: "ts_2".to_string(),
        tool: "get_terminal_context".to_string(),
        summary: String::new(),
    };
    match roundtrip_resp(&resp) {
        Response::ToolStarted { summary, .. } => assert!(summary.is_empty()),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_tool_finished_roundtrip() {
    let resp = Response::ToolFinished {
        id: "tf_1".to_string(),
        ok: true,
        elapsed_ms: 432,
        detail: Some("42 lines".to_string()),
    };
    match roundtrip_resp(&resp) {
        Response::ToolFinished {
            id,
            ok,
            elapsed_ms,
            detail,
        } => {
            assert_eq!(id, "tf_1");
            assert!(ok);
            assert_eq!(elapsed_ms, 432);
            assert_eq!(detail.as_deref(), Some("42 lines"));
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_tool_finished_failed_no_detail_roundtrip() {
    let resp = Response::ToolFinished {
        id: "tf_2".to_string(),
        ok: false,
        elapsed_ms: 12,
        detail: None,
    };
    match roundtrip_resp(&resp) {
        Response::ToolFinished { ok, detail, .. } => {
            assert!(!ok);
            assert!(detail.is_none());
        }
        _ => panic!("wrong variant"),
    }
}

#[test]
fn response_tool_finished_backward_compat_no_detail_field() {
    // Old daemons that omit the `detail` field should deserialize with detail=None.
    let json = r#"{"ToolFinished":{"id":"tf_3","ok":true,"elapsed_ms":100}}"#;
    let parsed: Response = serde_json::from_str(json).expect("backward-compat deserialize");
    match parsed {
        Response::ToolFinished { id, ok, detail, .. } => {
            assert_eq!(id, "tf_3");
            assert!(ok);
            assert!(detail.is_none());
        }
        _ => panic!("wrong variant"),
    }
}
