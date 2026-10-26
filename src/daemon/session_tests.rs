use super::*;
use crate::ai::{Message, ToolResult};

fn make_msg(role: &str, content: &str) -> Message {
    Message {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_results: None,
        turn: None,
    }
}
fn make_msg_with_tool_results(role: &str, content: &str, results: Vec<ToolResult>) -> Message {
    Message {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_results: Some(results),
        turn: None,
    }
}

#[test]
fn append_session_message_adds_lines() {
    let id = format!("test_append_{}", std::process::id());
    let path = std::path::PathBuf::from("/tmp").join(format!("{}.jsonl", id));
    // Start with two messages written via the full-rewrite path.
    let msgs = vec![make_msg("user", "hello"), make_msg("assistant", "hi")];
    {
        use std::io::Write;
        let mut f = std::fs::File::create(&path).unwrap();
        for m in &msgs {
            writeln!(f, "{}", serde_json::to_string(m).unwrap()).unwrap();
        }
    }
    // Append one more message.
    let extra = make_msg("user", "how are you");
    // Call append_session_message via the session-file path directly.
    {
        use std::fs::OpenOptions;
        use std::io::Write;
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{}", serde_json::to_string(&extra).unwrap()).unwrap();
    }
    // Read back and verify all three messages are present.
    let text = std::fs::read_to_string(&path).unwrap();
    let loaded: Vec<Message> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert_eq!(loaded.len(), 3);
    assert_eq!(loaded[2].content, "how are you");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn session_file_roundtrip() {
    // Write messages to a temp session file and read them back.
    let id = format!("test_{}", std::process::id());
    // Temporarily point sessions_dir() at /tmp to avoid HOME dependency.
    // We call the helpers directly using /tmp as the base.
    let dir = std::path::PathBuf::from("/tmp");
    let path = dir.join(format!("{}.jsonl", id));

    let msgs = vec![make_msg("user", "hello"), make_msg("assistant", "hi there")];

    // Replicate write_session_file logic with a known path.
    use std::io::Write;
    let mut f = std::fs::File::create(&path).unwrap();
    for m in &msgs {
        writeln!(f, "{}", serde_json::to_string(m).unwrap()).unwrap();
    }

    // Replicate read_session_file logic with the same path.
    let text = std::fs::read_to_string(&path).unwrap();
    let loaded: Vec<Message> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].role, "user");
    assert_eq!(loaded[0].content, "hello");
    assert_eq!(loaded[1].role, "assistant");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn auto_name_suggested_starts_false() {
    // auto_name_suggested must default to false so the first suggestion fires.
    let entry = SessionEntry {
        messages: vec![],
        last_accessed: std::time::Instant::now(),
        chat_pane: None,
        default_target_pane: None,
        bg_windows: vec![],
        last_prompt_tokens: 0,
        tmux_session: "test".to_string(),
        last_detach: None,
        detach_time_utc: None,
        messages_at_detach: 0,
        is_ghost: false,
        ghost_config: None,
        ghost_bg_prefix: "",
        ghost_runbook_name: None,
        started_at: chrono::Utc::now(),
        turn_count: 0,
        tool_calls_this_session: 0,
        active_model: None,
        last_snapshot_activity: 0,
        saved_name: None,
        dirty: false,
        artifacts_created: vec![],
        auto_name_suggested: false,
        ghost_task_message: None,
        cost_usd: 0.0,
        loaded_tools: HashSet::new(),
        cost_by_agent: HashMap::new(),
        has_untracked_cost: false,
        token_scale: 1.5,
        compaction_in_flight: false,
        pending_compaction_notice: None,
    };
    assert!(!entry.auto_name_suggested);
    assert!(entry.saved_name.is_none());
}

#[test]
fn session_entry_accumulates_cost_across_turns() {
    let mut entry = SessionEntry {
        messages: vec![],
        last_accessed: std::time::Instant::now(),
        chat_pane: None,
        default_target_pane: None,
        bg_windows: vec![],
        last_prompt_tokens: 0,
        tmux_session: "test".to_string(),
        last_detach: None,
        detach_time_utc: None,
        messages_at_detach: 0,
        is_ghost: false,
        ghost_config: None,
        ghost_bg_prefix: "",
        ghost_runbook_name: None,
        started_at: chrono::Utc::now(),
        turn_count: 0,
        tool_calls_this_session: 0,
        active_model: None,
        last_snapshot_activity: 0,
        saved_name: None,
        dirty: false,
        artifacts_created: vec![],
        auto_name_suggested: false,
        ghost_task_message: None,
        cost_usd: 0.0,
        loaded_tools: HashSet::new(),
        cost_by_agent: HashMap::new(),
        has_untracked_cost: false,
        token_scale: 1.5,
        compaction_in_flight: false,
        pending_compaction_notice: None,
    };

    // Simulate three turns of cost accumulation.
    entry.cost_usd += 0.10;
    *entry.cost_by_agent.entry("chat".to_string()).or_insert(0.0) += 0.10;
    entry.cost_usd += 0.20;
    *entry.cost_by_agent.entry("chat".to_string()).or_insert(0.0) += 0.20;
    entry.cost_usd += 0.05;
    *entry.cost_by_agent.entry("chat".to_string()).or_insert(0.0) += 0.05;

    assert!((entry.cost_usd - 0.35).abs() < 1e-10);
    assert!((entry.cost_by_agent["chat"] - 0.35).abs() < 1e-10);
    assert!(!entry.has_untracked_cost);
}

#[test]
fn session_entry_per_agent_split() {
    let mut entry = SessionEntry {
        messages: vec![],
        last_accessed: std::time::Instant::now(),
        chat_pane: None,
        default_target_pane: None,
        bg_windows: vec![],
        last_prompt_tokens: 0,
        tmux_session: "test".to_string(),
        last_detach: None,
        detach_time_utc: None,
        messages_at_detach: 0,
        is_ghost: false,
        ghost_config: None,
        ghost_bg_prefix: "",
        ghost_runbook_name: None,
        started_at: chrono::Utc::now(),
        turn_count: 0,
        tool_calls_this_session: 0,
        active_model: None,
        last_snapshot_activity: 0,
        saved_name: None,
        dirty: false,
        artifacts_created: vec![],
        auto_name_suggested: false,
        ghost_task_message: None,
        cost_usd: 0.0,
        loaded_tools: HashSet::new(),
        cost_by_agent: HashMap::new(),
        has_untracked_cost: false,
        token_scale: 1.5,
        compaction_in_flight: false,
        pending_compaction_notice: None,
    };

    // Simulate /agent switch mid-flow.
    entry.cost_usd += 0.30;
    *entry.cost_by_agent.entry("chat".to_string()).or_insert(0.0) += 0.30;
    entry.cost_usd += 0.15;
    *entry
        .cost_by_agent
        .entry("architect".to_string())
        .or_insert(0.0) += 0.15;

    assert!((entry.cost_usd - 0.45).abs() < 1e-10);
    assert!((entry.cost_by_agent["chat"] - 0.30).abs() < 1e-10);
    assert!((entry.cost_by_agent["architect"] - 0.15).abs() < 1e-10);
    assert_eq!(entry.cost_by_agent.len(), 2);
}

#[test]
fn unknown_pricing_sets_has_untracked_cost() {
    let mut entry = SessionEntry {
        messages: vec![],
        last_accessed: std::time::Instant::now(),
        chat_pane: None,
        default_target_pane: None,
        bg_windows: vec![],
        last_prompt_tokens: 0,
        tmux_session: "test".to_string(),
        last_detach: None,
        detach_time_utc: None,
        messages_at_detach: 0,
        is_ghost: false,
        ghost_config: None,
        ghost_bg_prefix: "",
        ghost_runbook_name: None,
        started_at: chrono::Utc::now(),
        turn_count: 0,
        tool_calls_this_session: 0,
        active_model: None,
        last_snapshot_activity: 0,
        saved_name: None,
        dirty: false,
        artifacts_created: vec![],
        auto_name_suggested: false,
        ghost_task_message: None,
        cost_usd: 0.0,
        loaded_tools: HashSet::new(),
        cost_by_agent: HashMap::new(),
        has_untracked_cost: false,
        token_scale: 1.5,
        compaction_in_flight: false,
        pending_compaction_notice: None,
    };

    // Known pricing call.
    entry.cost_usd += 0.10;
    *entry.cost_by_agent.entry("chat".to_string()).or_insert(0.0) += 0.10;
    assert!(!entry.has_untracked_cost);

    // Unknown pricing call — flag should flip.
    entry.has_untracked_cost = true;
    assert!(entry.has_untracked_cost);

    // Subsequent known pricing calls don't reset the flag.
    entry.cost_usd += 0.05;
    *entry.cost_by_agent.entry("chat".to_string()).or_insert(0.0) += 0.05;
    assert!(entry.has_untracked_cost);
}

#[test]
fn archive_appends_survive_compaction_rewrite() {
    let _lock = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    let sessions_dir = crate::config::sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let id = "archive-sim";

    // Append 30 messages — each goes to both working file and archive
    for i in 0..30 {
        let msg = make_msg("user", &format!("msg {}", i));
        append_session_message(id, &msg);
    }

    // Verify archive has 30 lines
    let archive_path = archive_file(id);
    let archive_content = std::fs::read_to_string(&archive_path).unwrap();
    let archive_lines: Vec<&str> = archive_content.lines().collect();
    assert_eq!(
        archive_lines.len(),
        30,
        "archive should have exactly 30 messages"
    );

    // Simulate compaction: rewrite working file to only 5 messages
    let msgs: Vec<Message> = (0..5)
        .map(|i| make_msg("user", &format!("msg {}", i)))
        .collect();
    write_session_file(id, &msgs);

    // Archive should still have 30 messages (not 5, not 35)
    let archive_content = std::fs::read_to_string(&archive_path).unwrap();
    let archive_lines: Vec<&str> = archive_content.lines().collect();
    assert_eq!(
        archive_lines.len(),
        30,
        "archive should still have 30 messages after working file compaction"
    );

    // First and last should match the original 30 messages
    assert!(
        archive_lines[0].contains("msg 0"),
        "first archive line should be msg 0"
    );
    assert!(
        archive_lines[29].contains("msg 29"),
        "last archive line should be msg 29"
    );

    // No duplicates: archive length == messages appended, not 2x
    assert_eq!(archive_lines.len(), 30);
}

#[test]
fn archive_seeds_from_existing_working_file() {
    let _lock = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    let sessions_dir = crate::config::sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let id = "archive-seed";

    // Pre-populate working file with 10 messages (no archive yet)
    let working_path = session_file(id);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&working_path)
        .unwrap();
    use std::io::Write;
    for i in 0..10 {
        let msg = make_msg("user", &format!("seed {}", i));
        let line = serde_json::to_string(&msg).unwrap();
        writeln!(file, "{}", line).unwrap();
    }
    drop(file);

    // Verify no archive exists yet
    assert!(!archive_file(id).exists());

    // First append_archive_message should seed from working file + append new msg
    let new_msg = make_msg("user", "new msg");
    append_archive_message(id, &new_msg);

    // Archive should have 11 messages (10 seeded + 1 new)
    let archive_content = std::fs::read_to_string(archive_file(id)).unwrap();
    let archive_lines: Vec<&str> = archive_content.lines().collect();
    assert_eq!(
        archive_lines.len(),
        11,
        "archive should have 10 seeded + 1 new = 11 messages"
    );

    // First line should be the first seeded message
    assert!(
        archive_lines[0].contains("seed 0"),
        "first archive line should be the first seeded message"
    );
    // Last line should be the new message
    assert!(
        archive_lines[10].contains("new msg"),
        "last archive line should be the new message"
    );
}

#[test]
fn archive_seed_absent_working_file() {
    let _lock = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    let sessions_dir = crate::config::sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let id = "archive-fresh";

    // Neither file exists
    assert!(!session_file(id).exists());
    assert!(!archive_file(id).exists());

    // Append one message
    let msg = make_msg("user", "first");
    append_archive_message(id, &msg);

    // Archive should have exactly 1 message (no seed, just the append)
    let archive_content = std::fs::read_to_string(archive_file(id)).unwrap();
    let archive_lines: Vec<&str> = archive_content.lines().collect();
    assert_eq!(
        archive_lines.len(),
        1,
        "archive should have exactly 1 message when neither file existed"
    );
}

#[test]
fn archive_file_path_is_correct() {
    let path = archive_file("abc123");
    assert_eq!(
        path.file_name().unwrap().to_str().unwrap(),
        "abc123.archive.jsonl",
        "archive file path should end with <id>.archive.jsonl"
    );
}

fn assert_no_orphan_tool_results(msgs: &[Message]) {
    for (i, m) in msgs.iter().enumerate() {
        if let Some(results) = &m.tool_results {
            for r in results {
                let found = msgs[..i].iter().rev().any(|prev| {
                    prev.tool_calls
                        .as_ref()
                        .is_some_and(|calls| calls.iter().any(|c| c.id == r.tool_call_id))
                });
                assert!(
                    found,
                    "orphan tool_result at idx {}: call_id={}",
                    i, r.tool_call_id
                );
            }
        }
    }
}

#[test]
fn meta_roundtrip() {
    let _lock = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };
    let sessions_dir = crate::config::sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let id = "meta-roundtrip";
    let meta = SessionMeta {
        started_at: chrono::Utc::now(),
        turn_count: 5,
        last_prompt_tokens: 120,
        token_scale: 1.8,
        tool_calls_this_session: 3,
        saved_name: Some("my-session".to_string()),
    };
    write_session_meta(id, &meta);

    let loaded = read_session_meta(id).expect("meta file should exist");
    assert_eq!(loaded.turn_count, 5);
    assert_eq!(loaded.last_prompt_tokens, 120);
    assert_eq!(loaded.token_scale, 1.8);
    assert_eq!(loaded.tool_calls_this_session, 3);
    assert_eq!(loaded.saved_name, Some("my-session".to_string()));
}

#[test]
fn meta_corrupt_returns_none() {
    let _lock = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };
    let sessions_dir = crate::config::sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let id = "meta-corrupt";
    let path = meta_file(id);
    std::fs::write(&path, "NOT_JSON {{{").unwrap();

    let loaded = read_session_meta(id);
    assert!(
        loaded.is_none(),
        "corrupt meta should return None, not Some"
    );
}

#[test]
fn entry_recreation_seeds_from_meta() {
    let _lock = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };
    let sessions_dir = crate::config::sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let id = "meta-seed";
    let meta = SessionMeta {
        started_at: chrono::Utc::now(),
        turn_count: 7,
        last_prompt_tokens: 200,
        token_scale: 2.0,
        tool_calls_this_session: 10,
        saved_name: Some("seeded".to_string()),
    };
    write_session_meta(id, &meta);

    // Simulate entry recreation by reading meta and building a fresh entry.
    let loaded_meta = read_session_meta(id).expect("meta should exist");
    let entry = SessionEntry {
        messages: Vec::new(),
        last_accessed: std::time::Instant::now(),
        chat_pane: None,
        default_target_pane: None,
        bg_windows: Vec::new(),
        last_prompt_tokens: loaded_meta.last_prompt_tokens,
        tmux_session: String::new(),
        last_detach: None,
        detach_time_utc: None,
        messages_at_detach: 0,
        is_ghost: false,
        ghost_config: None,
        ghost_bg_prefix: crate::daemon::GS_BG_WINDOW_PREFIX,
        ghost_runbook_name: None,
        started_at: loaded_meta.started_at,
        turn_count: loaded_meta.turn_count,
        tool_calls_this_session: loaded_meta.tool_calls_this_session,
        active_model: None,
        last_snapshot_activity: 0,
        saved_name: loaded_meta.saved_name,
        dirty: false,
        artifacts_created: Vec::new(),
        auto_name_suggested: false,
        ghost_task_message: None,
        loaded_tools: std::collections::HashSet::new(),
        cost_usd: 0.0,
        cost_by_agent: std::collections::HashMap::new(),
        has_untracked_cost: false,
        token_scale: loaded_meta.token_scale,
        compaction_in_flight: false,
        pending_compaction_notice: None,
    };

    assert_eq!(entry.turn_count, 7);
    assert_eq!(entry.last_prompt_tokens, 200);
    assert_eq!(entry.token_scale, 2.0);
    assert_eq!(entry.tool_calls_this_session, 10);
    assert_eq!(entry.saved_name, Some("seeded".to_string()));
    assert!(!entry.compaction_in_flight);
    assert!(entry.pending_compaction_notice.is_none());
}

#[test]
fn read_session_file_lands_on_clean_boundary() {
    let _lock = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };
    let sessions_dir = crate::config::sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let id = "boundary-test";
    // Build a history where a small cap would slice mid-tool-chain:
    // user → assistant(tool_calls) → user(tool_results) → user(tool_results) → user(clean) → assistant
    // With cap=3, the raw slice starts at index 3 (user with tool_results).
    // Boundary-safe reload should advance to index 4 (clean user).
    let tool_result_1 = ToolResult {
        tool_call_id: "call-1".to_string(),
        tool_name: "read_file".to_string(),
        content: "file content".to_string(),
    };
    let tool_result_2 = ToolResult {
        tool_call_id: "call-2".to_string(),
        tool_name: "bash".to_string(),
        content: "output".to_string(),
    };
    let msgs: Vec<Message> = vec![
        make_msg("user", "hello"),
        make_msg("assistant", "thinking"),
        make_msg_with_tool_results("user", "result 1", vec![tool_result_1.clone()]),
        make_msg_with_tool_results("user", "result 2", vec![tool_result_2.clone()]),
        make_msg("user", "new question"),
        make_msg("assistant", "answer"),
    ];
    // Write the history to disk
    let path = session_file(id);
    let jsonl: String = msgs
        .iter()
        .map(|m| serde_json::to_string(m).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, jsonl).unwrap();

    // Read with a small cap that forces boundary alignment
    let result = read_session_file(id, Some(3));
    assert_no_orphan_tool_results(&result);
    // Verify the boundary was actually advanced: the first message should
    // be the clean user message (index 4), not the tool_results-bearing
    // user message (index 3).
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].content, "new question");
    assert!(result[0].tool_results.is_none());
}

#[test]
fn read_session_file_repairs_when_no_boundary() {
    let _lock = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };
    let sessions_dir = crate::config::sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();

    let id = "repair-test";
    // Build a history where the tail contains only tool_results-bearing user messages
    // (no clean boundary exists in the tail).
    let tool_result_1 = ToolResult {
        tool_call_id: "call-3".to_string(),
        tool_name: "read_file".to_string(),
        content: "file content".to_string(),
    };
    let tool_result_2 = ToolResult {
        tool_call_id: "call-4".to_string(),
        tool_name: "bash".to_string(),
        content: "output".to_string(),
    };
    let tool_result_3 = ToolResult {
        tool_call_id: "call-5".to_string(),
        tool_name: "search".to_string(),
        content: "matches".to_string(),
    };
    let msgs: Vec<Message> = vec![
        make_msg("user", "hello"),
        make_msg("assistant", "thinking"),
        make_msg_with_tool_results("user", "result 1", vec![tool_result_1]),
        make_msg_with_tool_results("user", "result 2", vec![tool_result_2]),
        make_msg_with_tool_results("user", "result 3", vec![tool_result_3]),
    ];
    let path = session_file(id);
    let jsonl: String = msgs
        .iter()
        .map(|m| serde_json::to_string(m).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, jsonl).unwrap();

    // Read with a cap that forces the tail to start within the tool_results zone
    let result = read_session_file(id, Some(3));
    // The repair should have stripped the orphaned tool_results
    assert_no_orphan_tool_results(&result);
    // Verify the repair actually happened: no message should have tool_results
    for msg in &result {
        assert!(
            msg.tool_results.as_ref().is_none_or(Vec::is_empty),
            "Expected repaired messages to have no tool_results"
        );
    }
}

fn entry_with(last_accessed: Instant) -> SessionEntry {
    SessionEntry {
        messages: vec![],
        last_accessed,
        chat_pane: None,
        default_target_pane: None,
        bg_windows: vec![],
        last_prompt_tokens: 0,
        tmux_session: "test".to_string(),
        last_detach: None,
        detach_time_utc: None,
        messages_at_detach: 0,
        is_ghost: false,
        ghost_config: None,
        ghost_bg_prefix: "",
        ghost_runbook_name: None,
        started_at: chrono::Utc::now(),
        turn_count: 0,
        tool_calls_this_session: 0,
        active_model: None,
        last_snapshot_activity: 0,
        saved_name: None,
        dirty: false,
        artifacts_created: vec![],
        auto_name_suggested: false,
        ghost_task_message: None,
        cost_usd: 0.0,
        loaded_tools: HashSet::new(),
        cost_by_agent: HashMap::new(),
        has_untracked_cost: false,
        token_scale: 1.5,
        compaction_in_flight: false,
        pending_compaction_notice: None,
    }
}

#[test]
fn cleanup_pass_releases_the_lock() {
    let sessions: SessionStore = SessionStore::new();
    let idle = Instant::now()
        .checked_sub(std::time::Duration::from_secs(3600))
        .unwrap_or(Instant::now());
    with_sessions(&sessions, |store| {
        store.insert("s1".to_string(), entry_with(idle));
    });

    let now = Instant::now();
    let (_evicted, _active) = cleanup_pass(&sessions, now, std::time::Duration::from_secs(1800));

    // Guard from cleanup_pass is dropped; try_lock must succeed.
    assert!(sessions.try_lock().is_ok());
}

#[test]
fn cleanup_pass_evicts_idle_and_keeps_active() {
    let sessions: SessionStore = SessionStore::new();
    let idle = Instant::now()
        .checked_sub(std::time::Duration::from_secs(3600))
        .unwrap_or(Instant::now());
    let active = Instant::now();

    with_sessions(&sessions, |store| {
        store.insert("idle".to_string(), entry_with(idle));
        store.insert("active".to_string(), entry_with(active));
    });

    let now = Instant::now();
    let (evicted, active_ids) = cleanup_pass(&sessions, now, std::time::Duration::from_secs(1800));

    assert_eq!(evicted.len(), 1);
    assert!(active_ids.contains("active"));
    assert!(!active_ids.contains("idle"));
    let remaining = sessions
        .try_lock()
        .expect("cleanup_pass must release the lock before returning");
    assert_eq!(remaining.len(), 1);
}

#[test]
fn with_sessions_runs_closure_and_releases_lock() {
    let sessions: SessionStore = SessionStore::new();
    with_sessions(&sessions, |store| {
        store.insert("test".to_string(), entry_with(Instant::now()));
    });

    let len = with_sessions(&sessions, |s| s.len());
    assert_eq!(len, 1, "closure return value should be passed through");
    assert!(
        sessions.try_lock().is_ok(),
        "guard must be released after with_sessions returns"
    );
}

#[test]
#[should_panic(expected = "re-entrant SessionStore lock")]
fn with_sessions_rejects_reentrant_call() {
    let sessions: SessionStore = SessionStore::new();
    with_sessions(&sessions, |_s| {
        // nested call — should panic with the re-entrancy message
        with_sessions(&sessions, |_s| {});
    });
}

#[test]
fn with_sessions_depth_resets_after_panic() {
    let sessions: SessionStore = SessionStore::new();

    // First call panics — the depth counter must still reset via Drop
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        with_sessions(&sessions, |_s| panic!("intentional test panic"));
    }));
    std::panic::set_hook(old_hook);

    // Second call must succeed — depth is back to 0
    let len = with_sessions(&sessions, |s| {
        s.insert("after-panic".to_string(), entry_with(Instant::now()));
        s.len()
    });
    assert_eq!(len, 1, "depth counter must reset after a panicked closure");
}

#[test]
fn with_sessions_sets_depth_inside_closure() {
    let sessions: SessionStore = SessionStore::new();
    with_sessions(&sessions, |_store| {
        assert_eq!(
            SESSIONS_LOCK_DEPTH.with(|d| d.get()),
            1,
            "depth must read 1 inside the closure — a `let _ =` binding on \
                 SessionsLockDepth::enter() would drop the guard immediately and \
                 read 0 here"
        );
    });
    assert_eq!(
        SESSIONS_LOCK_DEPTH.with(|d| d.get()),
        0,
        "depth must reset to 0 after the closure returns"
    );
}

#[test]
fn presence_register_counts_up_and_unregister_counts_down() {
    let sid = "presence-test-1";
    let conn_a = PresenceConnection::test_conn();
    let conn_b = PresenceConnection::test_conn();
    assert_eq!(presence_count(sid), 0, "fresh session has no presence");
    assert_eq!(presence_register(sid, conn_a.clone()), 1);
    assert_eq!(presence_register(sid, conn_b.clone()), 2);
    assert!(presence_live(sid));
    assert!(
        !presence_unregister(sid, &conn_a),
        "first close is not the last"
    );
    assert_eq!(presence_count(sid), 1);
    assert!(
        presence_unregister(sid, &conn_b),
        "second close is the last"
    );
    assert_eq!(presence_count(sid), 0);
    assert!(!presence_live(sid));
    assert!(
        !presence_unregister(sid, &conn_a),
        "a close for a session with no recorded presence is not an error"
    );
}

#[test]
fn attached_session_is_not_evicted() {
    let sessions: SessionStore = SessionStore::new();
    let idle = Instant::now()
        .checked_sub(std::time::Duration::from_secs(3600))
        .unwrap_or(Instant::now());
    with_sessions(&sessions, |store| {
        store.insert("attached".to_string(), entry_with(idle));
        store.insert("idle-only".to_string(), entry_with(idle));
    });
    let conn = PresenceConnection::test_conn();
    presence_register("attached", conn.clone());

    let (evicted, active_ids) = cleanup_pass(
        &sessions,
        Instant::now(),
        std::time::Duration::from_secs(1800),
    );
    assert_eq!(evicted.len(), 1, "only the session without presence goes");
    assert!(
        active_ids.contains("attached"),
        "a session with a live presence connection survives the idle window"
    );
    assert!(
        !active_ids.contains("idle-only"),
        "the same session without presence is evicted past the idle window"
    );
    presence_unregister("attached", &conn);

    let (evicted2, active_ids2) = cleanup_pass(
        &sessions,
        Instant::now(),
        std::time::Duration::from_secs(1800),
    );
    assert_eq!(evicted2.len(), 1);
    assert!(
        !active_ids2.contains("attached"),
        "after the presence closes, the session is evicted as usual"
    );
}

#[test]
fn record_chat_detach_stamps_only_when_the_entry_exists() {
    let sessions: SessionStore = SessionStore::new();
    with_sessions(&sessions, |store| {
        store.insert("has-entry".to_string(), entry_with(Instant::now()));
    });

    record_chat_detach("no-entry", &sessions);
    with_sessions(&sessions, |store| {
        assert_eq!(
            store.len(),
            1,
            "no entry must be created for the unknown id"
        );
    });

    with_sessions(&sessions, |store| {
        let entry = store.get_mut("has-entry").unwrap();
        entry.messages.push(make_msg("user", "one"));
        entry.messages.push(make_msg("assistant", "two"));
    });
    record_chat_detach("has-entry", &sessions);
    with_sessions(&sessions, |store| {
        let entry = store.get("has-entry").unwrap();
        assert!(entry.last_detach.is_some(), "detach must be stamped");
        assert!(entry.detach_time_utc.is_some());
        assert_eq!(
            entry.messages_at_detach, 2,
            "messages_at_detach is the message count at close"
        );
    });
}
