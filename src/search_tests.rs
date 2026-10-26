use super::*;
use std::env;

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct TmpHome(std::path::PathBuf);
impl TmpHome {
    fn new() -> Self {
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("de_srch_test_{}_{}", std::process::id(), n));
        std::fs::create_dir_all(&p).unwrap();
        TmpHome(p)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TmpHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_home() -> TmpHome {
    TmpHome::new()
}

fn with_home<F: FnOnce()>(tmp: &TmpHome, f: F) {
    let _guard = crate::test_home_guard();
    let old = env::var("HOME").ok();
    unsafe {
        env::set_var("HOME", tmp.path());
    }
    f();
    match old {
        Some(v) => unsafe {
            env::set_var("HOME", v);
        },
        None => unsafe {
            env::remove_var("HOME");
        },
    }
}

#[test]
fn search_finds_match_in_runbooks() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let dir = crate::config::config_dir().join("runbooks");
        std::fs::create_dir_all(&dir).unwrap();
        let content = "# Runbook: disk-check\n\n## Alert Criteria\n- disk usage above 90%\n";
        std::fs::write(dir.join("disk-check.md"), content).unwrap();

        // Index the runbook so FTS can find it
        crate::memory::index::index_artifact("runbook", "disk-check", "", content).unwrap();

        let results = search_repository("disk usage", "runbooks", 1);
        assert!(!results.is_empty());
        assert!(
            results
                .iter()
                .any(|r| r.matched_line.contains("disk usage"))
        );
    });
}

#[test]
fn search_returns_empty_for_no_match() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let dir = crate::config::config_dir().join("runbooks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("test.md"),
            "# Runbook: test\n\n## Alert Criteria\n- something\n",
        )
        .unwrap();

        let results = search_repository("xyzzy_not_found_12345", "runbooks", 1);
        assert!(results.is_empty());
    });
}

#[test]
fn search_respects_kind_filter() {
    let tmp = temp_home();
    with_home(&tmp, || {
        // Write a runbook with the keyword
        let rb_dir = crate::config::config_dir().join("runbooks");
        std::fs::create_dir_all(&rb_dir).unwrap();
        let rb_content = "# Runbook: needle\n\n## Alert Criteria\n- contains_needle\n";
        std::fs::write(rb_dir.join("needle.md"), rb_content).unwrap();

        // Index the runbook
        crate::memory::index::index_artifact("runbook", "needle", "", rb_content).unwrap();

        // Write a script without the keyword
        let sc_dir = crate::config::config_dir().join("scripts");
        std::fs::create_dir_all(&sc_dir).unwrap();
        std::fs::write(sc_dir.join("nope.sh"), "#!/bin/bash\necho nope").unwrap();

        // Search only scripts — should not find the runbook match
        let results = search_repository("contains_needle", "scripts", 0);
        assert!(
            results.is_empty(),
            "script search should not return runbook matches"
        );

        // Search runbooks — should find it
        let results = search_repository("contains_needle", "runbooks", 0);
        assert!(!results.is_empty());
    });
}

#[test]
fn search_events_returns_tail_not_head_when_segment_exceeds_cap() {
    let _lock = crate::test_home_guard();
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", dir.path()) };
    crate::config::Config::ensure_dirs().unwrap();

    let events_dir = crate::config::events_dir();
    std::fs::create_dir_all(&events_dir).unwrap();
    let seg = events_dir.join("events-20260101.jsonl");

    let mut lines = Vec::new();
    // Write 10,001 lines: the first 6 have a unique "EARLY_IDX" marker,
    // the rest are generic (no unique marker).
    for i in 0..EVENTS_TAIL_LINES {
        let ts = format!("2026-01-01T00:00:{:02}Z", i % 60);
        let line = format!(
            r#"{{"event":"test","ts":"{}","agent_name":"agent","provider":"anthropic","model":"claude","tokens":{{"input_tokens":1,"output_tokens":1,"cache_read_tokens":0,"cache_write_tokens":0}},"cost":{{"total_cost_usd":0.01}},"pricing_source":"BuiltinDefault","idx":{}}}"#,
            ts, i
        );
        lines.push(line);
    }
    // 5 tail lines — these are the most recent
    for i in 0..5 {
        let ts = format!("2026-01-01T01:00:{:02}Z", i);
        let line = format!(
            r#"{{"event":"test","ts":"{}","agent_name":"agent","provider":"anthropic","model":"claude","tokens":{{"input_tokens":1,"output_tokens":1,"cache_read_tokens":0,"cache_write_tokens":0}},"cost":{{"total_cost_usd":0.01}},"pricing_source":"BuiltinDefault","idx":{}}}"#,
            ts,
            EVENTS_TAIL_LINES + i
        );
        lines.push(line);
    }

    std::fs::write(&seg, lines.join("\n") + "\n").unwrap();

    // Search for a high index that only exists in the tail. `search_events`
    // matches against the `json_to_readable` form, which renders fields as
    // `key=value` (e.g. `idx=10003`) — not raw JSON — so the query must use
    // that form.
    let results = search_repository(&format!("idx={}", EVENTS_TAIL_LINES + 3), "events", 0);
    assert!(
        !results.is_empty(),
        "idx={} should be found in the tail of the segment",
        EVENTS_TAIL_LINES + 3
    );

    // idx=0 is the very first line, outside the last-EVENTS_TAIL_LINES tail
    // window — it must NOT be surfaced (this is the tail-not-head guarantee).
    let results = search_repository("idx=0", "events", 0);
    assert!(
        results.is_empty(),
        "idx=0 should not appear — only the last EVENTS_TAIL_LINES lines are searched"
    );
}

#[test]
fn memory_search_dirs_label_incidents_plural() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let incidents_dir = crate::config::config_dir().join("memory").join("incidents");
        std::fs::create_dir_all(&incidents_dir).unwrap();
        std::fs::write(
            incidents_dir.join("test-incident.md"),
            "# test incident\n\nThis is a test incident file.",
        )
        .unwrap();

        let results = search_repository("test incident", "memory", 0);
        assert!(!results.is_empty(), "should find the test incident file");

        // The label must be the plural directory name "memory/incidents"
        for r in &results {
            if r.kind.starts_with("memory/") {
                assert_eq!(
                    r.kind, "memory/incidents",
                    "label must be memory/incidents (plural), not memory/incident"
                );
            }
        }
    });
}

#[test]
fn stemmed_query_finds_runbook_with_root_word() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let dir = crate::config::config_dir().join("runbooks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("service-recovery.md"),
            "# Service Recovery\n\nThe service must restart cleanly after a crash.\n",
        )
        .unwrap();

        crate::memory::index::index_artifact(
            "runbook",
            "service-recovery",
            "",
            "The service must restart cleanly after a crash.",
        )
        .unwrap();

        let results = search_repository("restarting", "runbooks", 0);
        assert!(
            !results.is_empty(),
            "stemmed query 'restarting' should find runbook with 'restart'. Results: {:?}",
            results.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
        assert!(
            results.iter().any(|r| r.name == "service-recovery"),
            "should find service-recovery runbook"
        );
    });
}

#[test]
fn stemmed_hit_renders_a_non_empty_matched_line() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let dir = crate::config::config_dir().join("runbooks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("recovery-plan.md"),
            "# Recovery Plan\n\nThe system restarts automatically.\n",
        )
        .unwrap();

        crate::memory::index::index_artifact(
            "runbook",
            "recovery-plan",
            "",
            "The system restarts automatically.",
        )
        .unwrap();

        let results = search_repository("restarting", "runbooks", 0);
        assert!(!results.is_empty(), "stemmed hit should produce results");
        for r in &results {
            assert!(
                !r.matched_line.is_empty(),
                "matched_line must be non-empty for a stemmed-only hit"
            );
        }
    });
}

#[test]
fn stemmed_query_finds_memory_entry() {
    let tmp = temp_home();
    with_home(&tmp, || {
        crate::memory::add_memory(
            "daemon-behavior",
            "the daemon restarts on signal",
            crate::memory::MemoryCategory::Knowledge,
            "global",
        )
        .unwrap();

        let results = search_repository("restarting", "memory", 0);
        assert!(
            !results.is_empty(),
            "stemmed query 'restarting' should find memory with 'restarts'"
        );
        assert!(
            results.iter().any(|r| r.name == "daemon-behavior"),
            "should find daemon-behavior memory"
        );
    });
}

#[test]
fn stemmed_query_finds_script() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let dir = crate::config::config_dir().join("scripts");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("restart-service"),
            "#!/bin/bash\nsystemctl restart myservice\n",
        )
        .unwrap();

        crate::memory::index::index_artifact(
            "script",
            "restart-service",
            "",
            "#!/bin/bash\nsystemctl restart myservice",
        )
        .unwrap();

        let results = search_repository("restarting", "scripts", 0);
        assert!(
            !results.is_empty(),
            "stemmed query 'restarting' should find script with 'restart'"
        );
        assert!(
            results.iter().any(|r| r.name == "restart-service"),
            "should find restart-service script"
        );
    });
}

#[test]
fn events_kind_finds_webhook_alert_by_free_text() {
    let _lock = crate::test_home_guard();
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", dir.path()) };

    let events_dir = crate::config::events_dir();
    std::fs::create_dir_all(&events_dir).unwrap();
    // Real segments are `events-YYYYMMDD.jsonl`; the indexed label is the stem.
    let seg = events_dir.join("events-20260101.jsonl");
    let line = r#"{"event":"webhook_alert","ts":"2026-01-01T00:00:00Z","msg":"disk space critical on /dev/sda1"}"#;
    std::fs::write(&seg, format!("{line}\n")).unwrap();

    // Index it through the production hook so the body, masking and the
    // map/FTS insert order all match what log_event actually writes.
    crate::memory::index::index_event(
        "events-20260101",
        0,
        "webhook_alert",
        &crate::search::json_to_readable(line),
    )
    .unwrap();

    let results = search_repository("webhook_alert", "events", 0);
    assert!(
        !results.is_empty(),
        "events search should find webhook_alert by free text"
    );
}

#[test]
fn results_are_rank_ordered_not_alphabetical() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let dir = crate::config::config_dir().join("runbooks");
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("alpha.md"), "# Alpha\n\nquokka quokka quokka\n").unwrap();
        crate::memory::index::index_artifact("runbook", "alpha", "", "quokka quokka quokka")
            .unwrap();

        std::fs::write(
                dir.join("zebra.md"),
                "# Zebra\n\nthe quick brown fox jumps over the lazy dog many times and then quokka appears once at the end\n",
            )
            .unwrap();
        crate::memory::index::index_artifact(
                "runbook",
                "zebra",
                "",
                "the quick brown fox jumps over the lazy dog many times and then quokka appears once at the end",
            )
            .unwrap();

        let results = search_repository("quokka", "runbooks", 0);
        assert!(results.len() >= 2, "should find both runbooks");

        let names: Vec<&str> = results.iter().map(|r| r.name.as_str()).collect();
        assert!(
            names[0] == "alpha",
            "best-ranked document (alpha) should come first. Got: {:?}",
            names
        );
    });
}

#[test]
fn filename_match_still_returned_without_body_match() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let dir = crate::config::config_dir().join("runbooks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("deploy-checklist.md"),
            "# Checklist\n\nReview all items before release.\n",
        )
        .unwrap();

        let results = search_repository("deploy", "runbooks", 0);
        assert!(
            !results.is_empty(),
            "filename match should still return even without index hit"
        );
        assert!(
            results.iter().any(|r| r.name == "deploy-checklist"),
            "should find deploy-checklist by filename"
        );
    });
}

#[test]
fn file_matching_name_and_body_appears_once() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let dir = crate::config::config_dir().join("runbooks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            // Exactly ONE body line contains "deploy" so the count below is a
            // real de-dup assertion: results are one-per-matching-line, so a
            // second matching line would legitimately make this 2.
            dir.join("deploy-guide.md"),
            "# Service Runbook\n\nFollow these steps to deploy the service.\n",
        )
        .unwrap();

        crate::memory::index::index_artifact(
            "runbook",
            "deploy-guide",
            "",
            "Follow these steps to deploy the service.",
        )
        .unwrap();

        let results = search_repository("deploy", "runbooks", 0);
        let deploy_count = results.iter().filter(|r| r.name == "deploy-guide").count();
        assert_eq!(
            deploy_count, 1,
            "file matching both by name and body should appear exactly once"
        );
    });
}

#[test]
fn non_matching_document_is_absent() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let dir = crate::config::config_dir().join("runbooks");
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(
            dir.join("target.md"),
            "# Target\n\nThe service must restart cleanly.\n",
        )
        .unwrap();
        crate::memory::index::index_artifact(
            "runbook",
            "target",
            "",
            "The service must restart cleanly.",
        )
        .unwrap();

        std::fs::write(
            dir.join("decoy.md"),
            "# Decoy\n\nThis is about cooking pasta.\n",
        )
        .unwrap();
        crate::memory::index::index_artifact(
            "runbook",
            "decoy",
            "",
            "This is about cooking pasta.",
        )
        .unwrap();

        let results = search_repository("restarting", "runbooks", 0);
        assert!(
            results.iter().any(|r| r.name == "target"),
            "should find target"
        );
        assert!(
            !results.iter().any(|r| r.name == "decoy"),
            "decoy must NOT appear in results"
        );
    });
}

#[test]
fn search_survives_unwritable_index() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let dir = crate::config::config_dir().join("runbooks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("deploy-checklist.md"),
            "# Checklist\n\nReview all items before release.\n",
        )
        .unwrap();

        let _ = crate::memory::index::open_index();
        let index_path = crate::config::memory_index_path();
        let index_dir = index_path.parent().unwrap();
        let original_perms = std::fs::metadata(index_dir).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(index_dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        }

        let results = search_repository("deploy", "runbooks", 0);
        assert!(
            results.iter().any(|r| r.name == "deploy-checklist"),
            "filename match should still work when index is unwritable"
        );

        std::fs::set_permissions(index_dir, original_perms).unwrap();
    });
}

#[test]
fn turns_kind_finds_archived_turn() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let session_id = format!(
            "test-sess-{}",
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let archive_path = crate::daemon::session::archive_file(&session_id);
        let line = r#"{"role":"user","content":"tell me about quokka recovery"}"#;
        std::fs::write(&archive_path, format!("{line}\n")).unwrap();

        crate::memory::index::index_turn(&session_id, 1, 0, "tell me about quokka recovery")
            .unwrap();

        let results = search_repository("quokka recovery", "turns", 0);
        assert!(
            !results.is_empty(),
            "turns search should find archived turn"
        );
        assert!(
            results
                .iter()
                .any(|r| r.name.contains(&session_id) && r.name.contains("turn 1")),
            "result name should contain session id and turn number. Got: {:?}",
            results.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
    });
}

#[test]
fn turns_hit_shows_tool_result_text() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let session_id = format!(
            "test-sess-tr-{}",
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let archive_path = crate::daemon::session::archive_file(&session_id);
        // Message with empty content but a tool_result containing the search term
        let line = r#"{"role":"assistant","content":"","tool_results":[{"tool_call_id":"tc1","tool_name":"disk_check","content":"disk usage at 95 percent on sda1"}]}"#;
        std::fs::write(&archive_path, format!("{line}\n")).unwrap();

        crate::memory::index::index_turn(&session_id, 1, 0, "disk usage at 95 percent on sda1")
            .unwrap();

        let results = search_repository("disk usage", "turns", 0);
        assert!(
            !results.is_empty(),
            "turns search should find a turn matching only in tool_results"
        );
        // The matched_line must include the tool result text
        let matched = results.iter().find(|r| r.kind == "turns");
        assert!(matched.is_some(), "should have a turns result");
        assert!(
            matched
                .unwrap()
                .matched_line
                .contains("disk usage at 95 percent"),
            "matched_line must include tool result text. Got: {:?}",
            results.iter().map(|r| &r.matched_line).collect::<Vec<_>>()
        );
    });
}

#[test]
fn epochs_kind_finds_narrative() {
    let _lock = crate::test_home_guard();
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", dir.path()) };
    crate::config::Config::ensure_dirs().unwrap();

    let session_id = format!(
        "test-sess-ep-{}",
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    crate::memory::index::index_epoch(
        &session_id,
        1,
        "compaction",
        "The system experienced a cascading failure during the quokka migration",
    )
    .unwrap();

    let results = search_repository("cascading failure", "epochs", 0);
    assert!(
        !results.is_empty(),
        "epochs search should find narrative by free text"
    );
    assert!(
        results
            .iter()
            .any(|r| r.name.contains(&session_id) && r.name.contains("epoch 1")),
        "result name should contain session id and seq. Got: {:?}",
        results.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
}

#[test]
fn turns_results_are_rank_ordered() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let session_id = format!(
            "test-sess-rank-t-{}",
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let archive_path = crate::daemon::session::archive_file(&session_id);
        // Write two lines: the weaker match first, the stronger match second.
        // The stronger match (written last) should be returned first.
        let weak = r#"{"role":"user","content":"the quick brown fox quokka"}"#;
        let strong = r#"{"role":"user","content":"quokka quokka quokka quokka quokka"}"#;
        let content = format!("{weak}\n{strong}\n");
        std::fs::write(&archive_path, &content).unwrap();

        let weak_offset = 0u64;
        let strong_offset = (weak.len() + 1) as u64;

        crate::memory::index::index_turn(&session_id, 1, weak_offset, "the quick brown fox quokka")
            .unwrap();
        crate::memory::index::index_turn(
            &session_id,
            2,
            strong_offset,
            "quokka quokka quokka quokka quokka",
        )
        .unwrap();

        let results = search_repository("quokka", "turns", 0);
        assert!(results.len() >= 2, "should find both turns");
        // The stronger match (turn 2, more occurrences of "quokka") should be first
        assert!(
            results[0].name.contains("turn 2"),
            "best-ranked turn (turn 2) should come first. Got: {:?}",
            results.iter().map(|r| &r.name).collect::<Vec<_>>()
        );
    });
}

#[test]
fn epochs_results_are_rank_ordered() {
    let _lock = crate::test_home_guard();
    let dir = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", dir.path()) };
    crate::config::Config::ensure_dirs().unwrap();

    let session_id = format!(
        "test-sess-rank-e-{}",
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    // Write the weaker match first, the stronger match last.
    crate::memory::index::index_epoch(&session_id, 1, "compaction", "the quick brown fox quokka")
        .unwrap();
    crate::memory::index::index_epoch(
        &session_id,
        2,
        "compaction",
        "quokka quokka quokka quokka quokka",
    )
    .unwrap();

    let results = search_repository("quokka", "epochs", 0);
    assert!(results.len() >= 2, "should find both epochs");
    // The stronger match (seq 2) should be first
    assert!(
        results[0].name.contains("epoch 2"),
        "best-ranked epoch (epoch 2) should come first. Got: {:?}",
        results.iter().map(|r| &r.name).collect::<Vec<_>>()
    );
}

#[test]
fn all_kind_excludes_turns_and_epochs() {
    let tmp = temp_home();
    with_home(&tmp, || {
        // Write a turn
        let session_id = format!(
            "test-sess-all-{}",
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let sessions_dir = crate::config::sessions_dir();
        std::fs::create_dir_all(&sessions_dir).unwrap();

        let archive_path = crate::daemon::session::archive_file(&session_id);
        let line = r#"{"role":"user","content":"needle in the haystack"}"#;
        std::fs::write(&archive_path, format!("{line}\n")).unwrap();
        crate::memory::index::index_turn(&session_id, 1, 0, "needle in the haystack").unwrap();

        // Write an epoch
        crate::memory::index::index_epoch(&session_id, 1, "compaction", "needle in the haystack")
            .unwrap();

        let results = search_repository("needle", "all", 0);
        for r in &results {
            assert!(
                r.kind != "turns",
                "kind='all' must NOT include turns. Found kind={}",
                r.kind
            );
            assert!(
                r.kind != "epochs",
                "kind='all' must NOT include epochs. Found kind={}",
                r.kind
            );
        }
    });
}

#[test]
fn turns_hit_with_missing_archive_is_skipped() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let session_id = format!(
            "test-sess-missing-{}",
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        // Index a turn but do NOT create the archive file
        crate::memory::index::index_turn(&session_id, 1, 0, "some content here").unwrap();

        // Should not panic — the missing archive is silently skipped
        let results = search_repository("some content", "turns", 0);
        assert!(
            results.is_empty(),
            "missing archive file should produce empty results, not a panic"
        );
    });
}

#[test]
fn new_kinds_survive_unwritable_index() {
    let tmp = temp_home();
    with_home(&tmp, || {
        let _ = crate::memory::index::open_index();
        let index_path = crate::config::memory_index_path();
        let index_dir = index_path.parent().unwrap();
        let original_perms = std::fs::metadata(index_dir).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(index_dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        }

        // Both new kinds should return empty (not panic) when index is unwritable
        let results = search_repository("anything", "turns", 0);
        assert!(
            results.is_empty(),
            "turns should return empty on unwritable index"
        );

        let results = search_repository("anything", "epochs", 0);
        assert!(
            results.is_empty(),
            "epochs should return empty on unwritable index"
        );

        std::fs::set_permissions(index_dir, original_perms).unwrap();
    });
}
