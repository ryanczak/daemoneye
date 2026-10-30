use super::*;
use std::io::{BufRead, Seek};

// ── Phase 03a tests ──────────────────────────────────────────────────────

fn make_test_message(role: &str, content: &str, turn: Option<usize>) -> crate::ai::Message {
    crate::ai::Message {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_results: None,
        turn,
    }
}

#[test]
fn append_archive_message_indexes_the_turn() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    let session_id = "idx-test";
    let msg = make_test_message("user", "hello from index test", Some(5));

    // Append without any prior working file — no seed
    crate::daemon::session::append_archive_message(session_id, &msg);

    // Verify the turn is searchable
    let conn = open_index().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM turns WHERE turns MATCH 'hello'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "appended turn should be searchable");

    // Verify the map row exists
    let map_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM turns_map WHERE session_id = ?1 AND turn = ?2",
            (session_id, 5),
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        map_count, 1,
        "turns_map should have one row for the appended turn"
    );
}

#[test]
fn appended_turn_offset_seeks_to_its_line() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    let session_id = "offset-test";
    let msg = make_test_message("user", "seekable content", Some(10));

    crate::daemon::session::append_archive_message(session_id, &msg);

    // Read the offset from the index
    let conn = open_index().unwrap();
    let offset: i64 = conn
        .query_row(
            "SELECT offset FROM turns_map WHERE session_id = ?1 AND turn = ?2",
            (session_id, 10),
            |r| r.get(0),
        )
        .unwrap();

    // Seek the archive to that offset and read the line
    let archive_path = crate::daemon::session::archive_file(session_id);
    let file = std::fs::File::open(&archive_path).unwrap();
    let mut reader = std::io::BufReader::new(file);
    reader
        .seek(std::io::SeekFrom::Start(offset as u64))
        .unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();

    assert!(
        line.contains("seekable content"),
        "seeking to offset {offset} should yield the appended line, got: {line}"
    );
}

#[test]
fn archive_seed_indexes_every_copied_line() {
    use std::io::Write;

    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    let session_id = "seed-test";
    let sessions_dir = crate::config::sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();

    // Create a working file with 3 messages (no archive yet)
    let working_path = crate::daemon::session::session_file(session_id);
    let mut f = std::fs::File::create(&working_path).unwrap();
    writeln!(f, r#"{{"role":"user","content":"first seeded","turn":1}}"#).unwrap();
    writeln!(
        f,
        r#"{{"role":"assistant","content":"second seeded","turn":1}}"#
    )
    .unwrap();
    writeln!(f, r#"{{"role":"user","content":"third seeded","turn":2}}"#).unwrap();
    drop(f);

    // Ensure no archive exists
    let archive_path = crate::daemon::session::archive_file(session_id);
    assert!(
        !archive_path.exists(),
        "archive should not exist before append"
    );

    // Append one more message — this triggers the seed + append
    let msg = make_test_message("user", "appended fourth", Some(3));
    crate::daemon::session::append_archive_message(session_id, &msg);

    // Verify: 3 seeded + 1 appended = 4 rows
    let conn = open_index().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM turns_map WHERE session_id = ?1",
            (session_id,),
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 4,
        "seeded archive (3 lines) + appended (1 line) = 4 turns rows"
    );

    // Verify each offset seeks to the right line.
    // The four rows in offset order are: first seeded, second seeded,
    // third seeded, appended fourth.
    let expected_in_order = [
        "first seeded",
        "second seeded",
        "third seeded",
        "appended fourth",
    ];

    let file = std::fs::File::open(&archive_path).unwrap();
    let mut reader = std::io::BufReader::new(file);

    let rows: Vec<(i64, i64)> = conn
        .prepare("SELECT turn, offset FROM turns_map WHERE session_id = ?1 ORDER BY offset ASC")
        .unwrap()
        .query_map((session_id,), |r| {
            Ok((r.get(0).unwrap(), r.get(1).unwrap()))
        })
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();

    // All offsets must be distinct
    let offsets: std::collections::HashSet<i64> = rows.iter().map(|(_, o)| *o).collect();
    assert_eq!(
        offsets.len(),
        rows.len(),
        "all offsets must be distinct, got {:?}",
        rows
    );

    for (i, (_turn, offset)) in rows.iter().enumerate() {
        reader
            .seek(std::io::SeekFrom::Start(*offset as u64))
            .unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(
            line.contains(expected_in_order[i]),
            "offset {offset} (row {i}) should contain '{expected}', got: {line}",
            expected = expected_in_order[i]
        );
    }
}

#[test]
fn message_without_turn_is_not_indexed() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    let session_id = "no-turn-test";
    let msg = make_test_message("user", "no turn number", None);

    crate::daemon::session::append_archive_message(session_id, &msg);

    let conn = open_index().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM turns_map WHERE session_id = ?1",
            (session_id,),
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "message with turn=None must not add a turns row");
}

#[test]
fn log_event_indexes_the_event() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    let fields = serde_json::json!({"msg": "test event content here"});
    crate::daemon::log_event("test_event", fields);

    let conn = open_index().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM events WHERE events MATCH 'test_event'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "event should be searchable immediately");

    // Verify segment label is the file stem
    let segment: String = conn
        .query_row(
            "SELECT segment FROM events_map ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        segment.starts_with("events-"),
        "segment label should be the file stem, got: {segment}"
    );
}

#[test]
fn log_event_offset_seeks_to_its_line() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    let fields = serde_json::json!({"msg": "seekable event"});
    crate::daemon::log_event("seek_test", fields);

    let conn = open_index().unwrap();
    let offset: i64 = conn
        .query_row(
            "SELECT offset FROM events_map ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();

    let path = crate::config::current_event_segment_path();
    let file = std::fs::File::open(&path).unwrap();
    let mut reader = std::io::BufReader::new(file);
    reader
        .seek(std::io::SeekFrom::Start(offset as u64))
        .unwrap();
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();

    assert!(
        line.contains("seekable event"),
        "seeking to offset {offset} should yield the event line, got: {line}"
    );
}

#[test]
fn append_epoch_indexes_the_narrative() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    let session_id = "epoch-idx-test";
    let rec = crate::daemon::context::epochs::EpochRecord {
        seq: 1,
        kind: "epoch".to_string(),
        turn_start: 0,
        turn_end: 10,
        ts_start: chrono::Utc::now(),
        ts_end: chrono::Utc::now(),
        msg_count: 5,
        narrative: Some("the daemon learned about indexing".to_string()),
        tally: crate::daemon::context::epochs::EpochTally::default(),
        artifacts: vec![],
        covers: None,
    };

    crate::daemon::context::epochs::append_epoch(session_id, &rec);

    let conn = open_index().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM epochs WHERE epochs MATCH 'indexing'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "epoch narrative should be searchable immediately");

    // Verify the map row
    let epoch_count: i64 = conn
        .query_row(
            "SELECT count(*) FROM epochs WHERE session_id = ?1 AND seq = ?2",
            (session_id, 1),
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(epoch_count, 1, "epochs table should have one row");
}

#[test]
fn rewriting_a_runbook_replaces_its_artifact_row() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    let name = "replace-me";
    let content1 = "---\ntags: [initial]\n---\n# Runbook: replace-me\n\n## Alert Criteria\n\nFirst version of the runbook.";
    let content2 = "---\ntags: [updated]\n---\n# Runbook: replace-me\n\n## Alert Criteria\n\nSecond version of the runbook.";

    crate::runbook::write_runbook(name, content1).unwrap();
    crate::runbook::write_runbook(name, content2).unwrap();

    let conn = open_index().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM artifacts WHERE kind = 'runbook' AND name = ?1",
            (name,),
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "rewriting a runbook must leave exactly one row, not two"
    );

    // Verify the content is the latest
    let body: String = conn
        .query_row(
            "SELECT body FROM artifacts WHERE kind = 'runbook' AND name = ?1",
            (name,),
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        body.contains("Second version"),
        "artifact body should reflect the latest write"
    );
}

#[test]
fn deleting_a_runbook_removes_its_artifact_row() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    let name = "delete-me";
    let content = "---\ntags: [cleanup]\n---\n# Runbook: delete-me\n\n## Alert Criteria\n\nThis runbook will be deleted.";

    crate::runbook::write_runbook(name, content).unwrap();

    let conn = open_index().unwrap();
    let before: i64 = conn
        .query_row(
            "SELECT count(*) FROM artifacts WHERE kind = 'runbook' AND name = ?1",
            (name,),
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(before, 1, "runbook should be indexed before deletion");

    crate::runbook::delete_runbook(name).unwrap();

    let conn = open_index().unwrap();
    let after: i64 = conn
        .query_row(
            "SELECT count(*) FROM artifacts WHERE kind = 'runbook' AND name = ?1",
            (name,),
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(after, 0, "deleted runbook should have no artifact row");
}

#[test]
fn incremental_and_reconcile_agree() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    // 1. Append an archived message
    let session_id = "agree-test";
    let msg = make_test_message("user", "incremental turn data", Some(1));
    crate::daemon::session::append_archive_message(session_id, &msg);

    // 2. Log an event
    let fields = serde_json::json!({"msg": "incremental event data"});
    crate::daemon::log_event("agree_event", fields);

    // 3. Append an epoch
    let rec = crate::daemon::context::epochs::EpochRecord {
        seq: 1,
        kind: "epoch".to_string(),
        turn_start: 0,
        turn_end: 5,
        ts_start: chrono::Utc::now(),
        ts_end: chrono::Utc::now(),
        msg_count: 3,
        narrative: Some("incremental epoch narrative".to_string()),
        tally: crate::daemon::context::epochs::EpochTally::default(),
        artifacts: vec![],
        covers: None,
    };
    crate::daemon::context::epochs::append_epoch(session_id, &rec);

    // 4. Write a runbook
    crate::runbook::write_runbook("agree-rb", "---\ntags: [test]\n---\n# Runbook: agree-rb\n\n## Alert Criteria\n\nincremental runbook body").unwrap();

    // 5. Write a script
    crate::scripts::write_script("agree.sh", "#!/bin/sh\necho incremental script body").unwrap();

    // Snapshot per-corpus counts from incremental writes
    let conn = open_index().unwrap();
    let turns_before: i64 = conn
        .query_row("SELECT count(*) FROM turns_map", [], |r| r.get(0))
        .unwrap();
    let events_before: i64 = conn
        .query_row("SELECT count(*) FROM events_map", [], |r| r.get(0))
        .unwrap();
    let epochs_before: i64 = conn
        .query_row("SELECT count(*) FROM epochs", [], |r| r.get(0))
        .unwrap();
    let artifacts_before: i64 = conn
        .query_row("SELECT count(*) FROM artifacts", [], |r| r.get(0))
        .unwrap();

    // 6. Run full reconcile
    let report = reconcile_index().expect("reconcile should succeed");

    // 7. Snapshot per-corpus counts after reconcile
    let conn = open_index().unwrap();
    let turns_after: i64 = conn
        .query_row("SELECT count(*) FROM turns_map", [], |r| r.get(0))
        .unwrap();
    let events_after: i64 = conn
        .query_row("SELECT count(*) FROM events_map", [], |r| r.get(0))
        .unwrap();
    let epochs_after: i64 = conn
        .query_row("SELECT count(*) FROM epochs", [], |r| r.get(0))
        .unwrap();
    let artifacts_after: i64 = conn
        .query_row("SELECT count(*) FROM artifacts", [], |r| r.get(0))
        .unwrap();

    assert_eq!(
        turns_before, turns_after,
        "turns count must agree: incremental={} reconcile={}",
        turns_before, turns_after
    );
    assert_eq!(
        events_before, events_after,
        "events count must agree: incremental={} reconcile={}",
        events_before, events_after
    );
    assert_eq!(
        epochs_before, epochs_after,
        "epochs count must agree: incremental={} reconcile={}",
        epochs_before, epochs_after
    );
    assert_eq!(
        artifacts_before, artifacts_after,
        "artifacts count must agree: incremental={} reconcile={}",
        artifacts_before, artifacts_after
    );

    // Reconcile should report no net change (rows_before == rows_after)
    assert_eq!(
        report.rows_before, report.rows_after,
        "reconcile after incremental writes should report no net change"
    );
}

#[test]
fn index_failure_does_not_break_append() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    // First, create the index so it exists
    let _ = open_index();

    // Make the index directory unwritable
    let index_path = crate::config::memory_index_path();
    let index_dir = index_path.parent().unwrap();
    let original_perms = std::fs::metadata(index_dir).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(index_dir, std::fs::Permissions::from_mode(0o000)).unwrap();

    // append_archive_message should still succeed (file write + best-effort index)
    let session_id = "fail-test";
    let msg = make_test_message("user", "resilient content", Some(1));
    crate::daemon::session::append_archive_message(session_id, &msg);

    // Verify the archive file was written despite the index being unwritable
    let archive_path = crate::daemon::session::archive_file(session_id);
    assert!(
        archive_path.exists(),
        "archive file should exist even when index is unwritable"
    );
    let content = std::fs::read_to_string(&archive_path).unwrap();
    assert!(
        content.contains("resilient content"),
        "archive should contain the appended message"
    );

    // Restore permissions for other tests
    std::fs::set_permissions(index_dir, original_perms).unwrap();
}

#[test]
fn index_failure_does_not_break_log_event() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    // Create the index first
    let _ = open_index();

    // Make the index directory unwritable
    let index_path = crate::config::memory_index_path();
    let index_dir = index_path.parent().unwrap();
    let original_perms = std::fs::metadata(index_dir).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(index_dir, std::fs::Permissions::from_mode(0o000)).unwrap();

    // log_event should still succeed (file write + best-effort index)
    let fields = serde_json::json!({"msg": "resilient event"});
    crate::daemon::log_event("resilient_event", fields);

    // Verify the event segment file was written
    let segment_path = crate::config::current_event_segment_path();
    assert!(
        segment_path.exists(),
        "event segment should exist even when index is unwritable"
    );
    let content = std::fs::read_to_string(&segment_path).unwrap();
    assert!(
        content.contains("resilient event"),
        "event segment should contain the logged event"
    );

    // Restore permissions for other tests
    std::fs::set_permissions(index_dir, original_perms).unwrap();
}

#[test]
fn corpus_from_table_resolves_known_tables() {
    assert_eq!(Corpus::from_table("memories"), Some(Corpus::Memories));
    assert_eq!(Corpus::from_table("artifacts"), Some(Corpus::Artifacts));
    assert_eq!(Corpus::from_table("epochs"), Some(Corpus::Epochs));
    assert_eq!(Corpus::from_table("turns"), Some(Corpus::Turns));
    assert_eq!(Corpus::from_table("events"), Some(Corpus::Events));
}

#[test]
fn corpus_from_table_rejects_map_and_unknown_tables() {
    assert_eq!(Corpus::from_table("turns_map"), None);
    assert_eq!(Corpus::from_table("events_map"), None);
    assert_eq!(Corpus::from_table("nonsense"), None);
    assert_eq!(Corpus::from_table(""), None);
}

#[test]
fn corpus_table_name_roundtrips() {
    for corpus in [
        Corpus::Memories,
        Corpus::Artifacts,
        Corpus::Epochs,
        Corpus::Turns,
        Corpus::Events,
    ] {
        assert_eq!(
            Corpus::from_table(corpus.table_name()),
            Some(corpus),
            "roundtrip failed for {:?}",
            corpus
        );
    }
}

#[test]
fn empty_corpus_search_preserves_other_corpora() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    // Index a turn
    let session_id = "test-sess-preserve";
    let sessions_dir = crate::config::sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();
    let archive_path = crate::daemon::session::archive_file(session_id);
    let line = r#"{"role":"user","content":"preserved turn content"}"#;
    std::fs::write(&archive_path, format!("{line}\n")).unwrap();
    index_turn(session_id, 1, 0, "preserved turn content").unwrap();

    // Index an epoch
    index_epoch(session_id, 1, "compaction", "preserved epoch content").unwrap();

    // Verify both are findable before the search
    let conn = open_index().unwrap();
    let turns_before: i64 = conn
        .query_row("SELECT count(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    let epochs_before: i64 = conn
        .query_row("SELECT count(*) FROM epochs", [], |r| r.get(0))
        .unwrap();
    assert!(turns_before > 0, "should have turn rows before search");
    assert!(epochs_before > 0, "should have epoch rows before search");

    // Search with kind="memory" — memories corpus is empty, so
    // open_and_reconcile_if_empty fires. With the fix, it only rebuilds
    // memories, not turns or epochs.
    let _results = crate::search::search_repository("anything", "memory", 0);

    // Verify both turn and epoch rows are still there
    let conn = open_index().unwrap();
    let turns_after: i64 = conn
        .query_row("SELECT count(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    let epochs_after: i64 = conn
        .query_row("SELECT count(*) FROM epochs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        turns_after, turns_before,
        "turn rows must be preserved after searching empty memory corpus"
    );
    assert_eq!(
        epochs_after, epochs_before,
        "epoch rows must be preserved after searching empty memory corpus"
    );
}

#[test]
fn all_kind_search_preserves_turns_and_epochs() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    // Index a turn
    let session_id = "test-sess-all-preserve";
    let sessions_dir = crate::config::sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();
    let archive_path = crate::daemon::session::archive_file(session_id);
    let line = r#"{"role":"user","content":"preserved all kind content"}"#;
    std::fs::write(&archive_path, format!("{line}\n")).unwrap();
    index_turn(session_id, 1, 0, "preserved all kind content").unwrap();

    // Index an epoch
    index_epoch(session_id, 1, "compaction", "preserved all kind epoch").unwrap();

    let conn = open_index().unwrap();
    let turns_before: i64 = conn
        .query_row("SELECT count(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    let epochs_before: i64 = conn
        .query_row("SELECT count(*) FROM epochs", [], |r| r.get(0))
        .unwrap();
    assert!(turns_before > 0);
    assert!(epochs_before > 0);

    // kind="all" chain hits memories first (empty) → per-corpus reconcile only
    let _results = crate::search::search_repository("anything", "all", 0);

    let conn = open_index().unwrap();
    let turns_after: i64 = conn
        .query_row("SELECT count(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    let epochs_after: i64 = conn
        .query_row("SELECT count(*) FROM epochs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        turns_after, turns_before,
        "turns preserved after all-kind search"
    );
    assert_eq!(
        epochs_after, epochs_before,
        "epochs preserved after all-kind search"
    );
}

#[test]
fn reconcile_corpus_rebuilds_only_its_own_corpus() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    // Seed a turn
    let session_id = "test-sess-reconcile-own";
    let sessions_dir = crate::config::sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();
    let archive_path = crate::daemon::session::archive_file(session_id);
    let line = r#"{"role":"user","content":"own corpus test"}"#;
    std::fs::write(&archive_path, format!("{line}\n")).unwrap();
    index_turn(session_id, 1, 0, "own corpus test").unwrap();

    // Seed an epoch
    index_epoch(session_id, 1, "compaction", "own corpus epoch").unwrap();

    let conn = open_index().unwrap();
    let turns_before: i64 = conn
        .query_row("SELECT count(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    let epochs_before: i64 = conn
        .query_row("SELECT count(*) FROM epochs", [], |r| r.get(0))
        .unwrap();
    assert!(turns_before > 0);
    assert!(epochs_before > 0);

    // Reconcile only memories — turns and epochs must be unchanged
    reconcile_corpus(Corpus::Memories).unwrap();

    let conn = open_index().unwrap();
    let turns_after: i64 = conn
        .query_row("SELECT count(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    let epochs_after: i64 = conn
        .query_row("SELECT count(*) FROM epochs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        turns_after, turns_before,
        "turns unchanged after memories reconcile"
    );
    assert_eq!(
        epochs_after, epochs_before,
        "epochs unchanged after memories reconcile"
    );
}

#[test]
fn reconcile_corpus_turns_clears_both_tables() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    let session_id = "test-sess-turns-clear";
    let sessions_dir = crate::config::sessions_dir();
    std::fs::create_dir_all(&sessions_dir).unwrap();
    let archive_path = crate::daemon::session::archive_file(session_id);
    // Include "turn" field so index_archive_file can parse it during reconcile
    let line = r#"{"role":"user","content":"turns clear test","turn":1}"#;
    std::fs::write(&archive_path, format!("{line}\n")).unwrap();
    index_turn(session_id, 1, 0, "turns clear test").unwrap();

    let conn = open_index().unwrap();
    let turns_before: i64 = conn
        .query_row("SELECT count(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    let turns_map_before: i64 = conn
        .query_row("SELECT count(*) FROM turns_map", [], |r| r.get(0))
        .unwrap();
    assert!(turns_before > 0, "should have turn rows");
    assert!(turns_map_before > 0, "should have turns_map rows");

    // Reconcile turns — should clear both tables and rebuild
    let count = reconcile_corpus(Corpus::Turns).unwrap();
    assert_eq!(
        count, turns_before as usize,
        "reconciled turns count matches"
    );

    let conn = open_index().unwrap();
    let turns_after: i64 = conn
        .query_row("SELECT count(*) FROM turns", [], |r| r.get(0))
        .unwrap();
    let turns_map_after: i64 = conn
        .query_row("SELECT count(*) FROM turns_map", [], |r| r.get(0))
        .unwrap();
    assert_eq!(turns_after, turns_before, "turns rebuilt to same count");
    assert_eq!(
        turns_map_after, turns_map_before,
        "turns_map rebuilt to same count"
    );
}

#[test]
fn reconcile_corpus_events_clears_both_tables() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    // Write an event segment
    let ev_dir = crate::config::events_dir();
    std::fs::create_dir_all(&ev_dir).unwrap();
    let ev_line = r#"{"event":"test_event_clear","ts":"2026-01-01T00:00:00Z"}"#;
    std::fs::write(ev_dir.join("events-20260101.jsonl"), format!("{ev_line}\n")).unwrap();
    index_event("events-20260101", 0, "test_event_clear", ev_line).unwrap();

    let conn = open_index().unwrap();
    let events_before: i64 = conn
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    let events_map_before: i64 = conn
        .query_row("SELECT count(*) FROM events_map", [], |r| r.get(0))
        .unwrap();
    assert!(events_before > 0, "should have event rows");
    assert!(events_map_before > 0, "should have events_map rows");

    let count = reconcile_corpus(Corpus::Events).unwrap();
    assert_eq!(
        count, events_before as usize,
        "reconciled events count matches"
    );

    let conn = open_index().unwrap();
    let events_after: i64 = conn
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    let events_map_after: i64 = conn
        .query_row("SELECT count(*) FROM events_map", [], |r| r.get(0))
        .unwrap();
    assert_eq!(events_after, events_before, "events rebuilt to same count");
    assert_eq!(
        events_map_after, events_map_before,
        "events_map rebuilt to same count"
    );
}

#[test]
fn reconcile_index_report_is_unchanged() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    // Seed one memory so the index isn't empty
    crate::memory::add_memory(
        "report-test",
        "report test body",
        crate::memory::MemoryCategory::Knowledge,
        "global",
    )
    .unwrap();

    let report = reconcile_index().unwrap();
    assert_eq!(
        report.per_corpus.len(),
        5,
        "report must have 5 corpus entries"
    );
    let names: Vec<&str> = report.per_corpus.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec!["memories", "artifacts", "epochs", "turns", "events"],
        "per_corpus order must be stable"
    );
    assert_eq!(
        report.rows_after, 1,
        "rows_after should match seeded memory"
    );
}

#[test]
fn unknown_table_name_reconciles_nothing() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    // Seed a memory
    crate::memory::add_memory(
        "unknown-table-guard",
        "unknown table guard body",
        crate::memory::MemoryCategory::Knowledge,
        "global",
    )
    .unwrap();

    let conn = open_index().unwrap();
    let mem_before: i64 = conn
        .query_row("SELECT count(*) FROM memories", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        mem_before, 1,
        "the seeded memory is indexed before the reconcile probe"
    );

    // open_and_reconcile_if_empty with an unknown table name must not
    // trigger any reconcile — the memory must survive
    let _conn = open_and_reconcile_if_empty("nonsense");

    let conn = open_index().unwrap();
    let mem_after: i64 = conn
        .query_row("SELECT count(*) FROM memories", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        mem_after, mem_before,
        "unknown table must not trigger reconcile"
    );
}

#[test]
fn empty_artifacts_corpus_still_self_heals() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    // Create a runbook on disk
    let rb_dir = crate::runbook::runbooks_dir();
    std::fs::create_dir_all(&rb_dir).unwrap();
    std::fs::write(rb_dir.join("self-heal.md"), "# Self Heal\n\nhealing body\n").unwrap();

    // The artifacts corpus is empty in the index. Searching it should
    // trigger a per-corpus reconcile that finds the runbook.
    let results = crate::search::search_repository("healing", "artifact", 0);
    assert!(
        results.iter().any(|r| r.name == "self-heal"),
        "self-healing artifacts corpus should find disk runbook"
    );
}

#[test]
fn category_filter_excludes_other_categories() {
    let _guard = crate::test_home_guard();
    let tmp = tempfile::tempdir().unwrap();
    unsafe { std::env::set_var("HOME", tmp.path()) };

    // Seed a knowledge memory and an incident memory, both matching "zephyr"
    crate::memory::add_memory(
        "zephyr-knowledge",
        "The zephyr blows softly",
        crate::memory::MemoryCategory::Knowledge,
        "global",
    )
    .expect("add knowledge memory");
    crate::memory::add_memory(
        "zephyr-incident",
        "The zephyr incident occurred",
        crate::memory::MemoryCategory::Incident,
        "global",
    )
    .expect("add incident memory");

    let unfiltered = fts5_search("zephyr", 10, &["global"]);
    assert_eq!(
        unfiltered.len(),
        2,
        "unfiltered search must return both knowledge and incident"
    );

    let filtered = fts5_search_in_category("zephyr", 10, &["global"], Some("incident"));
    assert_eq!(
        filtered.len(),
        1,
        "category filter must return only the incident"
    );
    assert_eq!(
        filtered[0].1, "zephyr-incident",
        "the one hit is the incident memory"
    );
}
